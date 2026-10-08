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

pub(crate) type SearchRequest = (PathBuf, SearchOptions, HashMap<PathBuf, ropey::Rope>, u64);
type FilterRequest = (u64, Arc<Vec<Entry>>, String);
/// One worker retains only the newest query; stale matching stops between files.
pub(crate) struct FilterWorker {
    request: Arc<(std::sync::Mutex<Option<FilterRequest>>, std::sync::Condvar)>,
    generation: Arc<std::sync::atomic::AtomicU64>,
}
impl FilterWorker {
    fn new(output: crate::services::ReplySender) -> Self {
        let request = Arc::new((
            std::sync::Mutex::new(None::<FilterRequest>),
            std::sync::Condvar::new(),
        ));
        let generation = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let inbox = request.clone();
        let latest = generation.clone();
        std::thread::spawn(move || loop {
            let (lock, changed) = &*inbox;
            let mut job = lock.lock().unwrap();
            while job.is_none() {
                job = changed.wait(job).unwrap();
            }
            let (mut gen, mut entries, mut query) = job.take().unwrap();
            drop(job);
            if gen == u64::MAX {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
            if let Some(next) = lock.lock().unwrap().take() {
                (gen, entries, query) = next;
            }
            if gen == u64::MAX {
                break;
            }
            let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
            let pattern = Pattern::parse(&query, CaseMatching::Smart, Normalization::Smart);
            let mut buffer = Vec::new();
            let mut scored = Vec::new();
            for (i, entry) in entries.iter().enumerate() {
                if i % 128 == 0 && latest.load(Ordering::Relaxed) != gen {
                    break;
                }
                let mut positions = Vec::new();
                if let Some(score) = pattern.indices(
                    Utf32Str::new(&entry.label, &mut buffer),
                    &mut matcher,
                    &mut positions,
                ) {
                    positions.sort_unstable();
                    positions.dedup();
                    scored.push((score, i, positions));
                }
            }
            if latest.load(Ordering::Relaxed) != gen {
                continue;
            }
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            if latest.load(Ordering::Relaxed) == gen
                && output
                    .send(Reply::Filtered(
                        gen,
                        scored.into_iter().map(|(_, i, p)| (i, p)).collect(),
                    ))
                    .is_err()
            {
                break;
            }
        });
        Self {
            request,
            generation,
        }
    }
    fn submit(&self, entries: Arc<Vec<Entry>>, query: String) -> u64 {
        let gen = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        *self.request.0.lock().unwrap() = Some((gen, entries, query));
        self.request.1.notify_one();
        gen
    }
}
impl Drop for FilterWorker {
    fn drop(&mut self) {
        self.generation.store(u64::MAX, Ordering::Relaxed);
        *self.request.0.lock().unwrap() = Some((u64::MAX, Arc::new(Vec::new()), String::new()));
        self.request.1.notify_one();
    }
}

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
    /// Items `first..first + items.len()` of `total`.
    pub items: Vec<PickerItem>,
    pub first: usize,
    /// The selected item, counted over all items.
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
    /// For search results: each file's content when it was searched.
    pub hashes: HashMap<PathBuf, u64>,
    /// The first item sent to frontends.
    pub first: usize,
    filter_source: Option<Arc<Vec<Entry>>>,
    filter_revision: u64,
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
            hashes: HashMap::new(),
            first: 0,
            filter_source: None,
            filter_revision: 0,
        }
    }
    /// Keep the selection inside the window of items frontends receive.
    pub fn keep_window(&mut self) {
        if self.selected < self.first {
            self.first = self.selected;
        } else if self.selected >= self.first + SHOWN {
            self.first = self.selected + 1 - SHOWN;
        }
        self.first = self.first.min(self.shown.len().saturating_sub(SHOWN));
    }
    /// Recompute `shown` from the query.
    pub fn filter(&mut self) {
        self.filter_source = None;
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
        self.first = 0;
        self.keep_window();
    }
    pub fn view(&self) -> PickerView {
        // A window of the list keeps long lists cheap to send.
        let items = self
            .shown
            .iter()
            .skip(self.first)
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
            first: self.first,
            selected: self.selected,
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
    fn filter_current_picker(&mut self) {
        let Some(p) = self.picker.as_mut().filter(|p| p.local) else {
            return;
        };
        if p.entries.len() < 1000 || p.query.trim().is_empty() {
            p.filter_revision = 0;
            p.filter();
            return;
        }
        let worker = self
            .fuzzy_worker
            .get_or_insert_with(|| FilterWorker::new(self.services.reply_sender()));
        let source = p
            .filter_source
            .get_or_insert_with(|| Arc::new(p.entries.clone()))
            .clone();
        p.filter_revision = worker.submit(source, p.query.clone());
        p.busy = true;
        p.shown.clear();
    }
    pub(crate) fn filtered_picker(&mut self, generation: u64, shown: Vec<(usize, Vec<u32>)>) {
        if let Some(p) = self
            .picker
            .as_mut()
            .filter(|p| p.filter_revision == generation && p.local)
        {
            p.shown = shown;
            p.busy = false;
            p.selected = 0;
            p.first = 0;
        }
    }

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
                // Replies are matched to the document they describe.
                p.generation = self.views[&id].document;
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
        if query.is_empty() {
            picker.filter();
        }
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
            p.filter_source = None;
        }
        self.filter_current_picker();
    }

    pub(crate) fn picker_query(&mut self, query: String) -> Result<()> {
        let Some(p) = self.picker.as_mut() else {
            return Ok(());
        };
        p.query = query;
        p.selected = 0;
        if p.local {
            self.filter_current_picker();
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
        let snapshots: HashMap<PathBuf, ropey::Rope> = self
            .documents
            .values()
            .filter(|d| d.dirty())
            .filter_map(|d| Some((d.path.clone()?, d.rope().clone())))
            .collect();
        let root = self.root.clone();
        let cancel = self.search_cancel.clone();
        let output = self.services.reply_sender();
        let worker = self.search_worker.get_or_insert_with(|| {
            crate::services::LatestWorker::new(
                move |(root, options, snapshots, generation): SearchRequest| {
                    if cancel.load(Ordering::Relaxed) != generation {
                        return;
                    }
                    let buffers = snapshots
                        .into_iter()
                        .map(|(path, rope)| (path, rope.to_string()))
                        .collect();
                    let result = crate::project::search(
                        &root,
                        &options,
                        &buffers,
                        &cancel,
                        generation,
                        |hits| output.send(Reply::SearchHits(generation, hits)).is_ok(),
                    );
                    let _ = output.send(Reply::SearchDone(generation, result));
                },
            )
        });
        worker.submit((root, options, snapshots, generation));
        Ok(())
    }

    pub(crate) fn search_hits(&mut self, generation: u64, hits: Vec<Hit>) {
        let root = self.root.clone();
        let Some(p) = self
            .picker
            .as_mut()
            .filter(|p| p.kind == "search" && p.generation == generation)
        else {
            return;
        };
        for hit in hits {
            p.hashes.insert(hit.path.clone(), hit.hash);
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
        let Some(p) = self
            .picker
            .as_mut()
            .filter(|p| p.kind == "search" && p.generation == generation)
        else {
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
                format!(
                    "First {n} results in {} · refine the search",
                    crate::counted(files, "file", "files")
                )
            }
            Ok(n) => format!(
                "{} in {}",
                crate::counted(n, "result", "results"),
                crate::counted(files, "file", "files")
            ),
            Err(e) => e,
        };
    }

    pub(crate) fn picker_move(&mut self, delta: i32) {
        if let Some(p) = self.picker.as_mut() {
            let last = p.shown.len().saturating_sub(1);
            p.selected = (p.selected as i64 + delta as i64).clamp(0, last as i64) as usize;
            p.keep_window();
        }
    }

    /// Show items from `first` on (a frontend scrolled the list).
    pub(crate) fn picker_window(&mut self, first: usize) {
        if let Some(p) = self.picker.as_mut() {
            p.first = first.min(p.shown.len().saturating_sub(SHOWN));
        }
    }

    /// Close the picker, stopping a search it started.
    pub(crate) fn close_picker(&mut self) {
        if self.picker.take().is_some_and(|p| p.kind == "search") {
            self.search_cancel.fetch_add(1, Ordering::SeqCst);
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
        // `index` counts over all items, so items arriving meanwhile
        // (a streaming search) do not change what was clicked.
        let chosen = index.unwrap_or(p.selected);
        let Some((entry, _)) = p.shown.get(chosen) else {
            bail!("Nothing selected");
        };
        let target = p.entries[*entry].target.clone();
        self.close_picker();
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
            ("Escape", _, _) => self.close_picker(),
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
        let Some(p) = self
            .picker
            .as_mut()
            .filter(|p| p.kind == "symbols" && p.generation == doc)
        else {
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
    /// Replace every project-search match. Open documents change in the
    /// editor (unsaved, undoable). Other files are rewritten on disk, unless
    /// they changed since the search or mix line endings (rewriting would
    /// normalise them); their previous contents are kept for
    /// undo-replace-in-files.
    pub(crate) fn replace_in_files(&mut self, replacement: &str) -> Result<()> {
        let options = self
            .last_search
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Search in files first"))?;
        let regex = options.compile().map_err(anyhow::Error::msg)?;
        let mut report = ReplaceReport::default();
        let mut closed: Vec<(PathBuf, Option<u64>)> = Vec::new();
        for (path, hash) in self.last_search_files.clone() {
            let Some(doc) = self.document_for(&path) else {
                closed.push((path, hash));
                continue;
            };
            let edits = replacements(
                &regex,
                options.regex,
                &self.documents[&doc].text(),
                replacement,
            );
            if edits.is_empty() {
                continue;
            }
            let title = self.documents[&doc].title();
            if self.documents[&doc].read_only {
                report.problems.push(format!("{title}: read-only"));
                continue;
            }
            match self.replace_in_document(doc, &edits) {
                Ok(_) => {
                    report.open_files += 1;
                    report.matches += edits.len();
                }
                Err(e) => report.problems.push(format!("{title}: {e:#}")),
            }
        }
        let replacement = replacement.to_string();
        self.services.background(move || {
            for (path, hash) in closed {
                // (replacements, content before, content after)
                type Rewrite = (usize, Vec<u8>, Vec<u8>);
                let result = (|| -> anyhow::Result<Option<Rewrite>> {
                    let before = std::fs::read(&path)?;
                    if hash.is_some_and(|h| h != crate::project::content_hash(&before)) {
                        anyhow::bail!("changed since the search; search again");
                    }
                    let mut doc = crate::document::Document::open(&path)?;
                    if doc.format.mixed_line_endings {
                        anyhow::bail!("mixed line endings; open it to replace there");
                    }
                    let edits = replacements(&regex, options.regex, &doc.text(), &replacement);
                    if edits.is_empty() {
                        return Ok(None);
                    }
                    doc.replace_many(&edits, 0)?;
                    doc.save(None)?;
                    let after = std::fs::read(&path)?;
                    Ok(Some((edits.len(), before, after)))
                })();
                match result {
                    Ok(None) => {}
                    Ok(Some((count, before, after))) => {
                        report.files += 1;
                        report.matches += count;
                        report.backups.push((path, before, after));
                    }
                    Err(e) => report.problems.push(format!("{}: {e:#}", path.display())),
                }
            }
            Reply::Replaced(report)
        });
        self.status = "Replacing in files…".into();
        Ok(())
    }

    pub(crate) fn replaced(&mut self, report: ReplaceReport) {
        let count = crate::counted;
        let mut status = format!(
            "Replaced {} in {}",
            count(report.matches, "match", "matches"),
            count(report.files + report.open_files, "file", "files")
        );
        if report.open_files > 0 {
            status.push_str(&format!(
                " · {} unsaved",
                count(report.open_files, "open document", "open documents")
            ));
        }
        if !report.backups.is_empty() {
            status.push_str(" · undo-replace-in-files restores files on disk");
        }
        if !report.problems.is_empty() {
            status.push_str(&format!(" · skipped {}", report.problems.join("; ")));
        }
        self.status = status;
        self.replace_backups = report.backups;
        self.index_at = None;
    }

    /// Put back the files the last replace-in-files rewrote, unless they
    /// changed again since.
    pub(crate) fn undo_replace_in_files(&mut self) -> Result<()> {
        let backups = std::mem::take(&mut self.replace_backups);
        if backups.is_empty() {
            bail!("No replace-in-files to undo");
        }
        let (mut restored, mut kept) = (0, Vec::new());
        for (path, before, after) in backups {
            if std::fs::read(&path).ok().as_deref() != Some(after.as_slice()) {
                kept.push(path.display().to_string());
                continue;
            }
            // Written in place, keeping the file's permissions and links.
            std::fs::write(&path, &before)?;
            restored += 1;
        }
        self.status = format!(
            "Restored {restored} file{}{}",
            if restored == 1 { "" } else { "s" },
            if kept.is_empty() {
                String::new()
            } else {
                format!(" · changed since, left alone: {}", kept.join(", "))
            }
        );
        Ok(())
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
                .map(|path| {
                    let hash = p.hashes.get(&path).copied();
                    (path, hash)
                })
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
        self.close_picker();
        self.prompt = Some(crate::actions::prompt("project-replace", String::new()));
        Ok(())
    }
}

/// The outcome of replace-in-files.
#[derive(Default)]
pub struct ReplaceReport {
    files: usize,
    open_files: usize,
    matches: usize,
    problems: Vec<String>,
    /// (file, previous bytes, written bytes) for undo.
    backups: Vec<(PathBuf, Vec<u8>, Vec<u8>)>,
}

/// The replacements for every match of `regex` in `text`. In regex mode
/// `$1`-style groups in `replacement` expand.
fn replacements(
    regex: &regex::Regex,
    expand: bool,
    text: &str,
    replacement: &str,
) -> Vec<(usize, usize, String)> {
    regex
        .captures_iter(text)
        .filter_map(|c| {
            let m = c.get(0)?;
            let mut value = String::new();
            if expand {
                c.expand(replacement, &mut value);
            } else {
                value.push_str(replacement);
            }
            (m.start() < m.end()).then_some((m.start(), m.end(), value))
        })
        .collect()
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
    #[test]
    fn large_fuzzy_picker_returns_only_the_latest_query() {
        let services = crate::services::Services::new(crate::events::Events::default());
        let entries: Arc<Vec<Entry>> = Arc::new(
            (0..200_000)
                .map(|i| Entry {
                    label: format!("src/file_{i:06}.rs"),
                    detail: String::new(),
                    kind: "file".into(),
                    target: Target::Document(i),
                })
                .collect(),
        );
        let worker = FilterWorker::new(services.reply_sender());
        for query in ["f", "file", "file_199"] {
            worker.submit(entries.clone(), query.into());
        }
        let generation = worker.submit(entries, "file_199999.rs".into());
        let until = Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Ok(Reply::Filtered(found, rows)) = services.rx.try_recv() {
                assert_eq!(found, generation);
                assert_eq!(rows.first().unwrap().0, 199999);
                break;
            }
            assert!(
                Instant::now() < until,
                "Large workspace filtering did not finish"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
}
