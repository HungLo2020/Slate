//! One picker for every list the user filters and jumps from: files in the
//! workspace, symbols, project search results, open documents, diagnostics
//! and language-server results. Frontends draw `PickerView` and send the
//! picker commands; typing, selection and acceptance live here.
use crate::{
    navigation::{Column, Location},
    project::{Hit, SearchOptions},
    services::Reply,
    App, Command,
};
use anyhow::{bail, Result};
use nucleo_matcher::{
    pattern::{CaseMatching, Normalization, Pattern},
    Config, Matcher, Utf32Str,
};
use serde::Serialize;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{atomic::Ordering, Arc},
    time::Instant,
};

/// Items sent to frontends; the rest are counted.
const SHOWN: usize = 500;
/// The workspace index is refreshed when older than this.
const INDEX_AGE: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct PickerItem {
    pub label: String,
    pub detail: String,
    pub kind: String,
    /// Character positions in `label` that matched the query.
    pub positions: Vec<u32>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct PickerView {
    pub kind: String,
    pub title: String,
    pub query: String,
    pub items: Vec<PickerItem>,
    pub selected: usize,
    pub total: usize,
    pub busy: bool,
    pub message: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

#[derive(Clone, Debug)]
pub(crate) enum Target {
    Location(Location),
    Document(u64),
    Command(Command),
}

#[derive(Clone, Debug)]
pub(crate) struct Entry {
    pub label: String,
    pub detail: String,
    pub kind: String,
    pub target: Target,
}

pub(crate) struct Picker {
    pub kind: String,
    pub title: String,
    pub query: String,
    pub selected: usize,
    pub entries: Vec<Entry>,
    /// Indices into `entries` with matched character positions, best first.
    pub shown: Vec<(usize, Vec<u32>)>,
    pub busy: bool,
    pub message: String,
    /// Filter `entries` locally (otherwise the source filters, as search).
    pub local: bool,
    pub options: SearchOptions,
    pub generation: u64,
}
impl Picker {
    pub fn new(kind: &str, title: &str, local: bool) -> Self {
        Self {
            kind: kind.into(),
            title: title.into(),
            query: String::new(),
            selected: 0,
            entries: Vec::new(),
            shown: Vec::new(),
            busy: false,
            message: String::new(),
            local,
            options: SearchOptions::default(),
            generation: 0,
        }
    }
    /// Recompute `shown` from the query.
    pub fn filter(&mut self) {
        if !self.local || self.query.trim().is_empty() {
            self.shown = (0..self.entries.len()).map(|i| (i, Vec::new())).collect();
        } else {
            let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
            let pattern = Pattern::parse(&self.query, CaseMatching::Smart, Normalization::Smart);
            let mut buffer = Vec::new();
            let mut scored: Vec<(u32, usize, Vec<u32>)> = self
                .entries
                .iter()
                .enumerate()
                .filter_map(|(i, e)| {
                    let haystack = Utf32Str::new(&e.label, &mut buffer);
                    let mut positions = Vec::new();
                    let score = pattern.indices(haystack, &mut matcher, &mut positions)?;
                    positions.sort_unstable();
                    positions.dedup();
                    Some((score, i, positions))
                })
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            self.shown = scored.into_iter().map(|(_, i, p)| (i, p)).collect();
        }
        self.selected = self.selected.min(self.shown.len().saturating_sub(1));
    }
    pub fn view(&self) -> PickerView {
        // Send a window around the selection so long lists stay cheap.
        let start = self
            .selected
            .saturating_sub(SHOWN / 2)
            .min(self.shown.len().saturating_sub(SHOWN));
        let items = self.shown[start..]
            .iter()
            .take(SHOWN)
            .map(|(i, positions)| {
                let e = &self.entries[*i];
                PickerItem {
                    label: e.label.clone(),
                    detail: e.detail.clone(),
                    kind: e.kind.clone(),
                    positions: positions.clone(),
                }
            })
            .collect();
        PickerView {
            kind: self.kind.clone(),
            title: self.title.clone(),
            query: self.query.clone(),
            items,
            selected: self.selected - start,
            total: self.shown.len(),
            busy: self.busy,
            message: self.message.clone(),
            case_sensitive: self.options.case_sensitive,
            whole_word: self.options.whole_word,
            regex: self.options.regex,
        }
    }
}

impl App {
    pub fn picker_view(&self) -> Option<PickerView> {
        self.picker.as_ref().map(Picker::view)
    }

    /// Open a picker: files, symbols, documents, search or diagnostics.
    pub(crate) fn open_picker(&mut self, kind: &str, query: &str) -> Result<()> {
        let mut picker = match kind {
            "files" => {
                let mut p = Picker::new("files", "Go to file", true);
                self.refresh_index();
                p.busy = self.index_pending;
                p.entries = self.index_entries();
                p
            }
            "symbols" => {
                let id = self
                    .active_editor()
                    .ok_or_else(|| anyhow::anyhow!("Focus an editor first"))?;
                let mut p = Picker::new("symbols", "Go to symbol", true);
                p.busy = true;
                self.request_symbols(id);
                p
            }
            "documents" => {
                let mut p = Picker::new("documents", "Open documents", true);
                p.entries = self
                    .documents
                    .iter()
                    .map(|(id, d)| Entry {
                        label: d.title(),
                        detail: d
                            .path
                            .as_ref()
                            .map(|p| self.relative(p))
                            .unwrap_or_default(),
                        kind: "document".into(),
                        target: Target::Document(*id),
                    })
                    .collect();
                p
            }
            "search" => {
                let mut p = Picker::new("search", "Search in files", false);
                p.message =
                    "Type to search the workspace · Alt+C case · Alt+W word · Alt+R regex".into();
                p
            }
            "diagnostics" => {
                let mut p = Picker::new("diagnostics", "Problems", true);
                p.entries = self.diagnostic_entries();
                if p.entries.is_empty() {
                    p.message = "No problems reported".into();
                }
                p
            }
            "workspace-symbols" => {
                let mut p = Picker::new("workspace-symbols", "Workspace symbols", false);
                p.message = "Type a symbol name".into();
                p
            }
            _ => bail!("Unknown picker: {kind}"),
        };
        picker.query = query.to_string();
        picker.filter();
        self.picker = Some(picker);
        if !query.is_empty() {
            self.picker_query(query.to_string())?;
        }
        Ok(())
    }

    fn relative(&self, path: &std::path::Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned()
    }

    fn index_entries(&self) -> Vec<Entry> {
        self.index
            .iter()
            .map(|relative| Entry {
                label: relative.clone(),
                detail: String::new(),
                kind: "file".into(),
                target: Target::Location(Location {
                    path: self.root.join(relative),
                    line: usize::MAX,
                    column: Column::Byte(0),
                }),
            })
            .collect()
    }

    /// Rebuild the workspace file index in the background when stale.
    pub(crate) fn refresh_index(&mut self) {
        if self.index_pending || self.index_at.is_some_and(|t| t.elapsed() < INDEX_AGE) {
            return;
        }
        self.index_pending = true;
        let root = self.root.clone();
        self.services
            .background(move || Reply::Index(root.clone(), crate::project::index(&root)));
    }

    pub(crate) fn index_ready(&mut self, root: PathBuf, files: Vec<String>) {
        self.index_pending = false;
        if root != self.root {
            return;
        }
        self.index = Arc::new(files);
        self.index_at = Some(Instant::now());
        let entries = self.index_entries();
        if let Some(p) = self.picker.as_mut().filter(|p| p.kind == "files") {
            p.entries = entries;
            p.busy = false;
            p.filter();
        }
    }

    pub(crate) fn picker_query(&mut self, query: String) -> Result<()> {
        let Some(p) = self.picker.as_mut() else {
            return Ok(());
        };
        p.query = query;
        p.selected = 0;
        if p.local {
            p.filter();
            return Ok(());
        }
        match p.kind.as_str() {
            "search" => self.start_search(),
            "workspace-symbols" => self.request_workspace_symbols(),
            _ => Ok(()),
        }
    }

    fn start_search(&mut self) -> Result<()> {
        let generation = self.search_cancel.fetch_add(1, Ordering::SeqCst) + 1;
        let Some(p) = self.picker.as_mut() else {
            return Ok(());
        };
        p.generation = generation;
        p.entries.clear();
        p.shown.clear();
        p.options.query = p.query.clone();
        if p.query.is_empty() {
            p.busy = false;
            p.message = "Type to search the workspace".into();
            return Ok(());
        }
        if let Err(e) = p.options.compile() {
            p.busy = false;
            p.message = e;
            return Ok(());
        }
        p.busy = true;
        p.message = String::new();
        let options = p.options.clone();
        // Unsaved documents are searched as they are in the editor.
        let buffers: HashMap<PathBuf, String> = self
            .documents
            .values()
            .filter(|d| d.dirty())
            .filter_map(|d| Some((d.path.clone()?, d.text())))
            .collect();
        let root = self.root.clone();
        let cancel = self.search_cancel.clone();
        let output = self.services.reply_sender();
        std::thread::spawn(move || {
            let result =
                crate::project::search(&root, &options, &buffers, &cancel, generation, |hits| {
                    output.send(Reply::SearchHits(generation, hits)).is_ok()
                });
            let _ = output.send(Reply::SearchDone(generation, result));
        });
        Ok(())
    }

    pub(crate) fn search_hits(&mut self, generation: u64, hits: Vec<Hit>) {
        let root = self.root.clone();
        let Some(p) = self.picker.as_mut().filter(|p| p.generation == generation) else {
            return;
        };
        for hit in hits {
            let relative = hit
                .path
                .strip_prefix(&root)
                .unwrap_or(&hit.path)
                .to_string_lossy()
                .into_owned();
            p.entries.push(Entry {
                label: hit.preview,
                detail: format!("{relative}:{}", hit.line + 1),
                kind: "match".into(),
                target: Target::Location(Location {
                    path: hit.path,
                    line: hit.line,
                    column: Column::Byte(hit.column),
                }),
            });
            p.shown.push((p.entries.len() - 1, Vec::new()));
        }
    }

    pub(crate) fn search_done(&mut self, generation: u64, result: Result<usize, String>) {
        let Some(p) = self.picker.as_mut().filter(|p| p.generation == generation) else {
            return;
        };
        p.busy = false;
        let files = p
            .entries
            .iter()
            .filter_map(|e| match &e.target {
                Target::Location(l) => Some(&l.path),
                _ => None,
            })
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        p.message = match result {
            Ok(0) => "No results".into(),
            Ok(n) if n >= crate::project::MATCH_LIMIT => {
                format!("First {n} results in {files} files · refine the search")
            }
            Ok(n) => format!("{n} results in {files} files"),
            Err(e) => e,
        };
    }

    pub(crate) fn picker_move(&mut self, delta: i32) {
        if let Some(p) = self.picker.as_mut() {
            let last = p.shown.len().saturating_sub(1);
            p.selected = (p.selected as i64 + delta as i64).clamp(0, last as i64) as usize;
        }
    }

    pub(crate) fn picker_option(&mut self, name: &str) -> Result<()> {
        let Some(p) = self.picker.as_mut().filter(|p| p.kind == "search") else {
            return Ok(());
        };
        match name {
            "case" => p.options.case_sensitive = !p.options.case_sensitive,
            "word" => p.options.whole_word = !p.options.whole_word,
            "regex" => p.options.regex = !p.options.regex,
            _ => bail!("Unknown search option: {name}"),
        }
        self.start_search()
    }

    pub(crate) fn picker_accept(&mut self, index: Option<usize>) -> Result<()> {
        let Some(p) = self.picker.as_ref() else {
            return Ok(());
        };
        // `index` counts within the items the frontend was sent.
        let start = p
            .selected
            .saturating_sub(SHOWN / 2)
            .min(p.shown.len().saturating_sub(SHOWN));
        let chosen = index.map_or(p.selected, |i| start + i);
        let Some((entry, _)) = p.shown.get(chosen) else {
            bail!("Nothing selected");
        };
        let target = p.entries[*entry].target.clone();
        self.picker = None;
        match target {
            Target::Location(mut location) => {
                if location.line == usize::MAX {
                    // A file without a position keeps its remembered caret.
                    if let Some(doc) = self.document_for(&location.path) {
                        self.remember_position();
                        self.reveal_document(doc);
                        return Ok(());
                    }
                    location.line = 0;
                }
                self.goto(location)
            }
            Target::Document(doc) => {
                if !self.documents.contains_key(&doc) {
                    bail!("That document was closed");
                }
                self.remember_position();
                self.reveal_document(doc);
                Ok(())
            }
            Target::Command(command) => self.execute(command),
        }
    }

    /// Keys while a picker is open. Returns true when the picker used it.
    pub(crate) fn picker_key(&mut self, k: &crate::Key) -> Result<bool> {
        if self.picker.is_none() {
            return Ok(false);
        }
        match (k.key.as_str(), k.ctrl, k.alt) {
            ("Escape", _, _) => self.picker = None,
            ("Enter", _, _) => self.picker_accept(None)?,
            ("Up", _, _) => self.picker_move(-1),
            ("Down", _, _) => self.picker_move(1),
            ("PageUp", _, _) => self.picker_move(-10),
            ("PageDown", _, _) => self.picker_move(10),
            ("Backspace", _, _) => {
                let mut query = self.picker.as_ref().unwrap().query.clone();
                query.pop();
                self.picker_query(query)?;
            }
            ("h", true, false) | ("H", true, false) => self.begin_replace_in_files()?,
            (key, false, true) if ["c", "w", "r"].contains(&key.to_lowercase().as_str()) => {
                let name = match key.to_lowercase().as_str() {
                    "c" => "case",
                    "w" => "word",
                    _ => "regex",
                };
                self.picker_option(name)?;
            }
            (_, false, false) if !k.text.is_empty() => {
                let mut query = self.picker.as_ref().unwrap().query.clone();
                query.push_str(&k.text.replace(['\n', '\r'], " "));
                self.picker_query(query)?;
            }
            _ => {}
        }
        Ok(true)
    }

    /// Outline the active document in the background.
    pub(crate) fn request_symbols(&mut self, id: u64) {
        let doc = self.views[&id].document;
        if self.lsp_document_symbols(doc) {
            return;
        }
        let d = &self.documents[&doc];
        let rope = d.rope().clone();
        let path = d.path.clone();
        self.services.background(move || {
            Reply::Outline(doc, crate::outline::symbols(&rope, path.as_deref()))
        });
    }

    pub(crate) fn symbols_ready(&mut self, doc: u64, symbols: Vec<crate::outline::Symbol>) {
        let Some(path) = self.documents.get(&doc).map(|d| d.path.clone()) else {
            return;
        };
        let Some(p) = self.picker.as_mut().filter(|p| p.kind == "symbols") else {
            return;
        };
        p.busy = false;
        p.entries = symbols
            .into_iter()
            .map(|s| Entry {
                label: s.name,
                detail: format!("{} · line {}", s.kind, s.line + 1),
                kind: s.kind,
                target: match &path {
                    Some(path) => Target::Location(Location {
                        path: path.clone(),
                        line: s.line,
                        column: Column::Byte(s.column),
                    }),
                    None => Target::Document(doc),
                },
            })
            .collect();
        if p.entries.is_empty() {
            p.message = "No symbols found".into();
        }
        p.filter();
    }

    /// Replace every project-search match. Open documents change in the
    /// editor (unsaved, undoable); other files are rewritten on disk.
    pub(crate) fn replace_in_files(&mut self, replacement: &str) -> Result<()> {
        let options = self
            .last_search
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Search in files first"))?;
        let regex = options.compile().map_err(anyhow::Error::msg)?;
        let mut changed_open = 0;
        let mut count = 0;
        let mut closed: Vec<PathBuf> = Vec::new();
        for path in self.last_search_files.clone() {
            if let Some(doc) = self.document_for(&path) {
                let text = self.documents[&doc].text();
                let edits: Vec<(usize, usize, String)> = regex
                    .captures_iter(&text)
                    .filter_map(|c| {
                        let m = c.get(0)?;
                        let mut value = String::new();
                        if options.regex {
                            c.expand(replacement, &mut value);
                        } else {
                            value.push_str(replacement);
                        }
                        (m.start() < m.end()).then_some((m.start(), m.end(), value))
                    })
                    .collect();
                if edits.is_empty() {
                    continue;
                }
                count += edits.len();
                changed_open += 1;
                let view = self.reveal_document(doc);
                let before = self.views[&view].cursor;
                self.documents
                    .get_mut(&doc)
                    .unwrap()
                    .replace_many(&edits, before)?;
                self.clamp_views(doc);
            } else {
                closed.push(path);
            }
        }
        let replacement = replacement.to_string();
        self.services.background(move || {
            let mut files = 0;
            let mut total = 0;
            let mut errors = Vec::new();
            for path in closed {
                let result = (|| -> anyhow::Result<usize> {
                    let mut doc = crate::document::Document::open(&path)?;
                    let text = doc.text();
                    let edits: Vec<(usize, usize, String)> = regex
                        .captures_iter(&text)
                        .filter_map(|c| {
                            let m = c.get(0)?;
                            let mut value = String::new();
                            if options.regex {
                                c.expand(&replacement, &mut value);
                            } else {
                                value.push_str(&replacement);
                            }
                            (m.start() < m.end()).then_some((m.start(), m.end(), value))
                        })
                        .collect();
                    if edits.is_empty() {
                        return Ok(0);
                    }
                    doc.replace_many(&edits, 0)?;
                    doc.save(None)?;
                    Ok(edits.len())
                })();
                match result {
                    Ok(0) => {}
                    Ok(n) => {
                        files += 1;
                        total += n;
                    }
                    Err(e) => errors.push(format!("{}: {e:#}", path.display())),
                }
            }
            Reply::Replaced(files, total, errors)
        });
        self.pending_replace = Some((changed_open, count));
        self.status = "Replacing in files…".into();
        Ok(())
    }

    pub(crate) fn replaced(&mut self, files: usize, total: usize, errors: Vec<String>) {
        let (open_files, open_count) = self.pending_replace.take().unwrap_or((0, 0));
        self.status = format!(
            "Replaced {} matches in {} files{}{}",
            total + open_count,
            files + open_files,
            if open_files > 0 {
                format!(" · {open_files} open documents changed (unsaved)")
            } else {
                String::new()
            },
            if errors.is_empty() {
                String::new()
            } else {
                format!(" · failed: {}", errors.join("; "))
            }
        );
        self.index_at = None;
    }

    /// Remember the accepted search so replace-in-files can repeat it.
    pub(crate) fn remember_search(&mut self) {
        if let Some(p) = self
            .picker
            .as_ref()
            .filter(|p| p.kind == "search" && !p.busy)
        {
            self.last_search = Some(p.options.clone());
            self.last_search_files = p
                .entries
                .iter()
                .filter_map(|e| match &e.target {
                    Target::Location(l) => Some(l.path.clone()),
                    _ => None,
                })
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
        }
    }

    /// Ctrl+H in the search picker: ask for the replacement text.
    pub(crate) fn begin_replace_in_files(&mut self) -> Result<()> {
        let searching = self.picker.as_ref().is_some_and(|p| p.kind == "search");
        if !searching {
            return self.open_picker("search", "").map(|_| {
                self.status = "Search first, then press Ctrl+H to replace the results".into();
            });
        }
        if self.picker.as_ref().is_some_and(|p| p.busy) {
            bail!("Wait for the search to finish");
        }
        self.remember_search();
        if self.last_search_files.is_empty() {
            bail!("No results to replace");
        }
        self.picker = None;
        self.prompt = Some(crate::actions::prompt("project-replace", String::new()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_filtering_ranks_and_marks_matches() {
        let mut p = Picker::new("files", "Files", true);
        for path in ["src/main.rs", "src/lib.rs", "docs/manual.md", "README.md"] {
            p.entries.push(Entry {
                label: path.into(),
                detail: String::new(),
                kind: "file".into(),
                target: Target::Document(0),
            });
        }
        p.query = "smain".into();
        p.filter();
        let view = p.view();
        assert_eq!(view.items[0].label, "src/main.rs");
        assert_eq!(view.items[0].positions, [0, 4, 5, 6, 7]);
        p.query = "md".into();
        p.filter();
        assert_eq!(p.view().total, 2);
        p.query = String::new();
        p.filter();
        assert_eq!(p.view().total, 4);
    }
}
