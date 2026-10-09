//! Workspace-wide file index and text search. Both walk the folder the way
//! Git sees it (`.gitignore`, `.ignore` and hidden files are skipped) on a
//! worker thread, with limits so huge trees stay responsive.
use anyhow::Context;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

/// Files indexed for quick open.
pub const INDEX_LIMIT: usize = 200_000;
/// Matches reported by one project search.
pub const MATCH_LIMIT: usize = 20_000;
/// Files larger than this are not searched.
const SEARCH_FILE_LIMIT: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchPolicy {
    pub include: String,
    pub exclude: String,
    pub hidden: bool,
    pub ignored: bool,
    pub max_bytes: u64,
}
impl Default for SearchPolicy {
    fn default() -> Self {
        Self {
            include: String::new(),
            exclude: String::new(),
            hidden: false,
            ignored: false,
            max_bytes: SEARCH_FILE_LIMIT,
        }
    }
}
impl SearchPolicy {
    pub fn validate(&self) -> Result<(), String> {
        if self.max_bytes == 0 || self.max_bytes > crate::document::MAX_FILE_BYTES as u64 {
            return Err("Search size limit must be between 1 byte and 1 GiB".into());
        }
        self.overrides(std::path::Path::new("."))?;
        Ok(())
    }
    fn overrides(&self, root: &Path) -> Result<ignore::overrides::Override, String> {
        let mut builder = ignore::overrides::OverrideBuilder::new(root);
        for glob in self.include.split(';').filter(|s| !s.trim().is_empty()) {
            builder.add(glob.trim()).map_err(|e| e.to_string())?;
        }
        for glob in self.exclude.split(';').filter(|s| !s.trim().is_empty()) {
            builder
                .add(&format!("!{}", glob.trim()))
                .map_err(|e| e.to_string())?;
        }
        builder.build().map_err(|e| e.to_string())
    }
}
#[derive(Default)]
pub struct SearchReport {
    pub matches: usize,
    pub large: usize,
    pub unreadable: usize,
    pub binary: usize,
}

fn search_walker(root: &Path, policy: &SearchPolicy) -> Result<ignore::WalkBuilder, String> {
    let mut builder = walker(root);
    builder
        .hidden(!policy.hidden)
        .git_ignore(!policy.ignored)
        .git_global(!policy.ignored)
        .git_exclude(!policy.ignored)
        .ignore(!policy.ignored)
        .overrides(policy.overrides(root)?);
    Ok(builder)
}

fn walker(root: &Path) -> ignore::WalkBuilder {
    let mut builder = ignore::WalkBuilder::new(root);
    builder
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .parents(true)
        .follow_links(false)
        // Ignore rules apply even outside a Git repository.
        .require_git(false);
    builder
}

/// Paths of the files under `root`, relative and `/`-separated, sorted.
pub fn index(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    for entry in walker(root).build().flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if let Ok(relative) = entry.path().strip_prefix(root) {
            files.push(relative.to_string_lossy().replace('\\', "/"));
            if files.len() >= INDEX_LIMIT {
                break;
            }
        }
    }
    files.sort();
    files
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchOptions {
    pub query: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}
impl SearchOptions {
    pub fn compile(&self) -> Result<Regex, String> {
        if self.query.is_empty() {
            return Err("Enter text to find".into());
        }
        let mut pattern = if self.regex {
            self.query.clone()
        } else {
            regex::escape(&self.query)
        };
        if self.whole_word {
            pattern = format!(r"\b(?:{pattern})\b");
        }
        regex::RegexBuilder::new(&pattern)
            .case_insensitive(!self.case_sensitive)
            .multi_line(true)
            .build()
            .map_err(|e| e.to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub path: PathBuf,
    /// Zero-based line.
    pub line: usize,
    /// Byte offset of the match within the line.
    pub column: usize,
    pub length: usize,
    /// The line, shortened around the match.
    pub preview: String,
    /// The searched file's content, so a later replace can tell whether the
    /// file changed since.
    pub hash: u64,
}

/// A fingerprint of file content (stable within one run of Slate).
pub fn content_hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// Shorten a line around a match for display.
fn preview(line: &str, column: usize, length: usize) -> (String, usize) {
    const CONTEXT: usize = 60;
    let line = line.trim_end_matches(['\n', '\r']);
    let mut start = column.saturating_sub(CONTEXT);
    while !line.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = column
        .saturating_add(length.min(240))
        .saturating_add(CONTEXT * 2)
        .min(line.len())
        .min(start + 480);
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    let leading = line[start..end].len() - line[start..end].trim_start().len();
    let leading = if leading <= column - start {
        leading
    } else {
        0
    };
    let start = if start == 0 { start + leading } else { start };
    let text = line[start..end].replace('\t', " ");
    (
        format!(
            "{}{text}{}",
            if start > 0 && leading == 0 { "…" } else { "" },
            if end < line.len() { "…" } else { "" }
        ),
        start,
    )
}

/// Search the workspace. `buffers` holds the text of open documents with
/// unsaved changes, searched instead of the file on disk. `emit` receives
/// batches and returns false to stop; the search also stops when `cancel`
/// moves past `generation`.
pub fn search(
    root: &Path,
    options: &SearchOptions,
    buffers: &HashMap<PathBuf, String>,
    cancel: &Arc<AtomicU64>,
    generation: u64,
    emit: impl FnMut(Vec<Hit>) -> bool,
) -> Result<usize, String> {
    search_with_policy(
        root,
        options,
        &SearchPolicy::default(),
        buffers,
        cancel,
        generation,
        emit,
    )
    .map(|r| r.matches)
}
#[allow(clippy::too_many_arguments)]
pub fn search_with_policy(
    root: &Path,
    options: &SearchOptions,
    policy: &SearchPolicy,
    buffers: &HashMap<PathBuf, String>,
    cancel: &Arc<AtomicU64>,
    generation: u64,
    emit: impl FnMut(Vec<Hit>) -> bool,
) -> Result<SearchReport, String> {
    search_source(
        root,
        options,
        policy,
        cancel,
        generation,
        |path| {
            buffers.get(path).map(|text| {
                if text.len() as u64 > policy.max_bytes {
                    Err(())
                } else {
                    Ok(text.clone())
                }
            })
        },
        emit,
    )
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn search_ropes(
    root: &Path,
    options: &SearchOptions,
    policy: &SearchPolicy,
    buffers: &HashMap<PathBuf, ropey::Rope>,
    cancel: &Arc<AtomicU64>,
    generation: u64,
    emit: impl FnMut(Vec<Hit>) -> bool,
) -> Result<SearchReport, String> {
    search_source(
        root,
        options,
        policy,
        cancel,
        generation,
        |path| {
            buffers.get(path).map(|rope| {
                if rope.len_bytes() as u64 > policy.max_bytes {
                    Err(())
                } else {
                    Ok(rope.to_string())
                }
            })
        },
        emit,
    )
}
#[allow(clippy::too_many_arguments)]
fn search_source(
    root: &Path,
    options: &SearchOptions,
    policy: &SearchPolicy,
    cancel: &Arc<AtomicU64>,
    generation: u64,
    mut buffer: impl FnMut(&Path) -> Option<Result<String, ()>>,
    mut emit: impl FnMut(Vec<Hit>) -> bool,
) -> Result<SearchReport, String> {
    policy.validate()?;
    let mut report = SearchReport::default();
    let regex = options.compile()?;
    let mut total = 0;
    let mut batch = Vec::new();
    let mut last_emit = std::time::Instant::now();
    for entry in search_walker(root, policy)?.build() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                report.unreadable += 1;
                continue;
            }
        };
        if cancel.load(Ordering::Relaxed) != generation {
            report.matches = total;
            return Ok(report);
        }
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path().to_path_buf();
        let (text, hash) = match buffer(&path) {
            Some(Ok(text)) => {
                let hash = content_hash(text.as_bytes());
                (text, hash)
            }
            Some(Err(())) => {
                report.large += 1;
                continue;
            }
            None => {
                if entry.metadata().map(|m| m.len()).unwrap_or(0) > policy.max_bytes {
                    report.large += 1;
                    continue;
                }
                use std::io::Read;
                let bytes = (|| -> std::io::Result<Vec<u8>> {
                    let mut bytes = Vec::new();
                    std::fs::File::open(&path)?
                        .take(policy.max_bytes + 1)
                        .read_to_end(&mut bytes)?;
                    Ok(bytes)
                })();
                let Ok(bytes) = bytes else {
                    report.unreadable += 1;
                    continue;
                };
                if bytes.len() as u64 > policy.max_bytes {
                    report.large += 1;
                    continue;
                }
                // Use the same BOM, legacy encoding and newline handling as
                // opened documents. UTF-16 NUL bytes are not binary data.
                let Ok(decoded) = crate::text_format::decode(&bytes, None) else {
                    report.binary += 1;
                    continue;
                };
                (decoded.text, content_hash(&bytes))
            }
        };
        let mut line = 0;
        let mut line_start = 0;
        for m in regex.find_iter(&text) {
            if m.start() == m.end() {
                continue;
            }
            // Lines and the line start advance from the previous match, so
            // a long file costs one pass however many matches it has.
            let between = &text[line_start..m.start()];
            if let Some(last) = between.rfind('\n') {
                line += between.matches('\n').count();
                line_start += last + 1;
            }
            let line_end = text[m.start()..]
                .find('\n')
                .map_or(text.len(), |i| m.start() + i);
            let column = m.start() - line_start;
            let length = m.end().min(line_end) - m.start();
            let (preview, _) = preview(&text[line_start..line_end], column, length);
            batch.push(Hit {
                path: path.clone(),
                line,
                column,
                length,
                preview,
                hash,
            });
            total += 1;
            if total >= MATCH_LIMIT {
                emit(std::mem::take(&mut batch));
                report.matches = total;
                return Ok(report);
            }
        }
        if !batch.is_empty() && (batch.len() >= 500 || last_emit.elapsed().as_millis() > 50) {
            if !emit(std::mem::take(&mut batch)) {
                report.matches = total;
                return Ok(report);
            }
            last_emit = std::time::Instant::now();
        }
    }
    if !batch.is_empty() {
        emit(batch);
    }
    report.matches = total;
    Ok(report)
}

impl crate::App {
    pub(crate) fn search_settings(&mut self, argument: &str) -> anyhow::Result<()> {
        if argument.is_empty() {
            self.prompt = Some(crate::search::Prompt {
                kind: "search-settings".into(),
                input: serde_json::to_string(&self.project_search_policy)?,
                ..Default::default()
            });
            return Ok(());
        }
        self.execute(crate::Command::ConfigureProjectSearch {
            policy: serde_json::from_str(argument).context(
                "Search settings require JSON with include, exclude, hidden, ignored and max_bytes",
            )?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn search_decodes_text_like_the_editor() {
        let dir = tempfile::tempdir().unwrap();
        let mut utf16 = vec![0xff, 0xfe];
        for unit in "café\r\nhello\r\n".encode_utf16() {
            utf16.extend_from_slice(&unit.to_le_bytes());
        }
        fs::write(dir.path().join("utf16.txt"), utf16).unwrap();
        fs::write(dir.path().join("legacy.txt"), b"caf\xe9\rhello\r").unwrap();
        fs::write(dir.path().join("binary.bin"), b"hello\0world").unwrap();
        for (query, line) in [("café", 0), ("hello", 1)] {
            let mut hits = Vec::new();
            search(
                dir.path(),
                &SearchOptions {
                    query: query.into(),
                    ..Default::default()
                },
                &HashMap::new(),
                &Arc::new(AtomicU64::new(0)),
                0,
                |batch| {
                    hits.extend(batch);
                    true
                },
            )
            .unwrap();
            assert_eq!(hits.len(), 2);
            assert!(hits.iter().all(|hit| hit.line == line && hit.column == 0));
        }
    }

    #[test]
    fn index_and_search_respect_ignore_rules_and_unsaved_buffers() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join(".gitignore"), "target/\n").unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {\n    hello();\n}\n").unwrap();
        fs::write(root.join("src/lib.rs"), "pub fn Hello() {}\n").unwrap();
        fs::write(root.join("target/out.rs"), "hello\n").unwrap();
        fs::write(root.join("blob.bin"), b"hello\0world").unwrap();
        fs::write(root.join(".hidden"), "hello").unwrap();
        assert_eq!(index(root), ["blob.bin", "src/lib.rs", "src/main.rs"]);

        let cancel = Arc::new(AtomicU64::new(1));
        let run = |options: &SearchOptions, buffers: &HashMap<PathBuf, String>| {
            let mut hits = Vec::new();
            search(root, options, buffers, &cancel, 1, |batch| {
                hits.extend(batch);
                true
            })
            .unwrap();
            hits.sort_by(|a, b| a.path.cmp(&b.path));
            hits
        };
        let options = SearchOptions {
            query: "hello".into(),
            ..Default::default()
        };
        let hits = run(&options, &HashMap::new());
        assert_eq!(hits.len(), 2, "{hits:?}");
        assert_eq!((hits[1].line, hits[1].column, hits[1].length), (1, 4, 5));
        assert_eq!(hits[1].preview, "hello();");
        let case = SearchOptions {
            case_sensitive: true,
            ..options.clone()
        };
        assert_eq!(run(&case, &HashMap::new()).len(), 1);
        // Unsaved text is searched instead of the file.
        let buffers = HashMap::from([(root.join("src/main.rs"), "nothing here".to_string())]);
        assert_eq!(run(&options, &buffers).len(), 1);
        let regex = SearchOptions {
            query: r"fn \w+\(".into(),
            regex: true,
            ..Default::default()
        };
        assert_eq!(run(&regex, &HashMap::new()).len(), 2);
        assert!(SearchOptions {
            query: "(".into(),
            regex: true,
            ..Default::default()
        }
        .compile()
        .is_err());
    }
}
