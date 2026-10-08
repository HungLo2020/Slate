//! Workspace-wide file index and text search. Both walk the folder the way
//! Git sees it (`.gitignore`, `.ignore` and hidden files are skipped) on a
//! worker thread, with limits so huge trees stay responsive.
use regex::Regex;
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
    let mut end = (column + length + CONTEXT * 2).min(line.len());
    while !line.is_char_boundary(end) {
        end += 1;
    }
    let leading = line[start..].len() - line[start..].trim_start().len();
    let start = if start == 0 { start + leading } else { start };
    let text = line[start..end].replace('\t', " ");
    (
        format!("{}{text}", if start > 0 && leading == 0 { "…" } else { "" }),
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
    mut emit: impl FnMut(Vec<Hit>) -> bool,
) -> Result<usize, String> {
    let regex = options.compile()?;
    let mut total = 0;
    let mut batch = Vec::new();
    let mut last_emit = std::time::Instant::now();
    for entry in walker(root).build().flatten() {
        if cancel.load(Ordering::Relaxed) != generation {
            return Ok(total);
        }
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path().to_path_buf();
        let (text, hash) = match buffers.get(&path) {
            Some(text) => (text.clone(), content_hash(text.as_bytes())),
            None => {
                if entry.metadata().map(|m| m.len()).unwrap_or(0) > SEARCH_FILE_LIMIT {
                    continue;
                }
                let Ok(bytes) = std::fs::read(&path) else {
                    continue;
                };
                // Binary files are skipped, as grep does.
                if bytes[..bytes.len().min(8192)].contains(&0) {
                    continue;
                }
                (
                    String::from_utf8_lossy(&bytes).into_owned(),
                    content_hash(&bytes),
                )
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
                return Ok(total);
            }
        }
        if !batch.is_empty() && (batch.len() >= 500 || last_emit.elapsed().as_millis() > 50) {
            if !emit(std::mem::take(&mut batch)) {
                return Ok(total);
            }
            last_emit = std::time::Instant::now();
        }
    }
    if !batch.is_empty() {
        emit(batch);
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
