//! Bounded document search and replacement; large ropes are searched off the UI thread.
use crate::{
    search::Search,
    services::{LatestWorker, Reply},
    App,
};
use anyhow::{bail, Context, Result};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

const INLINE_BYTES: usize = 256 * 1024;
const EDIT_LIMIT: usize = 10_000;
const REPLACEMENT_BYTES: usize = 16 * 1024 * 1024;
type Edits = Vec<(usize, usize, String)>;

#[derive(Clone)]
pub(crate) enum Mode {
    Find {
        backward: bool,
    },
    Statistics,
    Format(crate::text_format::TextFormat),
    Occurrences {
        selections: Vec<crate::cursors::Selection>,
        all: bool,
    },
    Replace {
        text: String,
        all: bool,
    },
}
#[derive(Clone)]
pub(crate) struct Request {
    ticket: u64,
    view: u64,
    doc: u64,
    version: u64,
    format: crate::text_format::TextFormat,
    rope: ropey::Rope,
    search: Search,
    cursor: usize,
    selected: Option<(usize, usize)>,
    mode: Mode,
}
pub(crate) enum Outcome {
    Found(usize, usize, usize, usize),
    Statistics(String),
    Format(crate::text_format::TextFormat),
    Replaced(Edits),
    Selected(Vec<crate::cursors::Selection>, bool),
}
#[derive(Default)]
pub(crate) struct State {
    pub(crate) busy: bool,
    ticket: Arc<AtomicU64>,
    worker: Option<LatestWorker<Request>>,
}
impl Drop for State {
    fn drop(&mut self) {
        self.cancel();
    }
}
impl State {
    pub(crate) fn cancel(&mut self) {
        self.ticket.fetch_add(1, Ordering::Relaxed);
        self.busy = false;
    }
}

fn run(request: &Request, latest: &AtomicU64) -> Result<Outcome> {
    if let Mode::Format(format) = &request.mode {
        crate::text_format::validate_rope(&request.rope, format)?;
        return Ok(Outcome::Format(format.clone()));
    }
    if matches!(request.mode, Mode::Statistics | Mode::Format(_)) {
        let (a, b) = request.selected.unwrap_or((0, request.rope.len_bytes()));
        let scope = if request.selected.is_some() {
            "Selection"
        } else {
            "Document"
        };
        let mut chars = 0;
        let mut words = 0;
        let mut lines = 0;
        let mut word = false;
        let mut last = '\n';
        for c in request.rope.byte_slice(a..b).chars() {
            if chars % 4096 == 0 && latest.load(Ordering::Relaxed) != request.ticket {
                bail!("Cancelled");
            }
            chars += 1;
            lines += usize::from(c == '\n');
            if !c.is_whitespace() && !word {
                words += 1;
            }
            word = !c.is_whitespace();
            last = c;
        }
        lines += usize::from(chars > 0 && last != '\n');
        return Ok(Outcome::Statistics(format!(
            "{scope}: {lines} lines, {words} words, {chars} characters"
        )));
    }
    let re = request.search.rope_regex()?;
    let (start, end) = request
        .search
        .scope
        .filter(|s| s.0 == request.doc)
        .map(|(_, a, b)| (a, b))
        .unwrap_or((0, request.rope.len_bytes()));
    let cancelled = || latest.load(Ordering::Relaxed) != request.ticket;
    let input = || regex_cursor::Input::new(&request.rope).range(start..end);
    match &request.mode {
        Mode::Statistics | Mode::Format(_) => unreachable!(),
        Mode::Find { backward } => {
            let mut first = None;
            let mut last = None;
            let mut candidate = None;
            let mut count = 0;
            for m in re.find_iter(input()) {
                if cancelled() {
                    bail!("Search cancelled");
                }
                count += 1;
                let found = (m.start(), m.end(), count);
                first.get_or_insert(found);
                last = Some(found);
                if *backward {
                    if m.end() <= request.cursor && request.selected != Some((m.start(), m.end())) {
                        candidate = Some(found);
                    }
                } else if candidate.is_none()
                    && m.start() >= request.cursor
                    && request.selected != Some((m.start(), m.end()))
                {
                    candidate = Some(found);
                    if request.rope.len_bytes() > INLINE_BYTES {
                        return Ok(Outcome::Found(m.start(), m.end(), 0, 0));
                    }
                }
            }
            let (a, b, index) = candidate
                .or(if *backward { last } else { first })
                .context("No matches")?;
            Ok(Outcome::Found(a, b, index, count))
        }
        Mode::Occurrences { selections, all } => {
            let mut found = Vec::new();
            let after = selections.iter().map(|s| s.range().1).max().unwrap_or(0);
            let mut wrapped = None;
            for m in re.find_iter(input()) {
                if cancelled() {
                    bail!("Search cancelled");
                }
                let s = crate::cursors::Selection {
                    cursor: m.end(),
                    anchor: Some(m.start()),
                };
                if *all {
                    found.push(s);
                    if found.len() > crate::cursors::MAX_CURSORS {
                        found.pop();
                        return Ok(Outcome::Selected(found, true));
                    }
                } else if !selections.iter().any(|old| old.range() == s.range()) {
                    wrapped.get_or_insert(s);
                    if m.start() >= after {
                        let mut next = selections.clone();
                        next.insert(0, s);
                        return Ok(Outcome::Selected(next, false));
                    }
                }
            }
            if !*all {
                let s = wrapped.context("No more occurrences")?;
                found = selections.clone();
                found.insert(0, s);
            }
            if let Some(index) = found
                .iter()
                .position(|s| Some(s.range()) == request.selected)
            {
                found.swap(0, index);
            }
            Ok(Outcome::Selected(found, false))
        }
        Mode::Replace { text, all } => {
            anyhow::ensure!(
                text.len() <= REPLACEMENT_BYTES,
                "Replacement exceeds 16 MiB; nothing was changed"
            );
            let tokens = regex::Regex::new(
                r"\$\$|\$\{(?P<braced>[A-Za-z0-9_]+)\}|\$(?P<bare>[A-Za-z0-9_]+)",
            )?;
            let mut edits = Vec::new();
            let mut bytes: usize = 0;
            let mut first = None;
            for caps in re.captures_iter(input()) {
                if cancelled() {
                    bail!("Search cancelled");
                }
                let m = caps.get_match().context("Invalid match")?;
                let selected = request.selected == Some((m.start(), m.end()));
                let mut replacement = String::new();
                if request.search.regex_mode {
                    let mut at = 0;
                    for token in tokens.captures_iter(text) {
                        let whole = token.get(0).unwrap();
                        replacement.push_str(&text[at..whole.start()]);
                        if whole.as_str() == "$$" {
                            replacement.push('$');
                        } else {
                            let name = token
                                .name("braced")
                                .or_else(|| token.name("bare"))
                                .unwrap()
                                .as_str();
                            let group = name
                                .parse::<usize>()
                                .ok()
                                .and_then(|n| caps.get_group(n))
                                .or_else(|| caps.get_group_by_name(name));
                            if let Some(group) = group {
                                anyhow::ensure!(
                                    replacement.len().saturating_add(group.end - group.start)
                                        <= REPLACEMENT_BYTES,
                                    "Replacement exceeds 16 MiB; nothing was changed"
                                );
                                replacement.push_str(
                                    &request.rope.byte_slice(group.start..group.end).to_string(),
                                );
                            }
                        }
                        at = whole.end();
                    }
                    replacement.push_str(&text[at..]);
                } else {
                    replacement.push_str(text);
                }
                anyhow::ensure!(
                    replacement.len() <= REPLACEMENT_BYTES,
                    "Replacement exceeds 16 MiB; nothing was changed"
                );
                let edit = (m.start(), m.end(), replacement);
                if !*all {
                    first.get_or_insert_with(|| edit.clone());
                    if selected || m.start() >= request.cursor {
                        return Ok(Outcome::Replaced(vec![edit]));
                    }
                    continue;
                }
                bytes = bytes
                    .checked_add(edit.2.len())
                    .context("Replacement is too large")?;
                if edits.len() >= EDIT_LIMIT || bytes > REPLACEMENT_BYTES {
                    bail!("Replace All exceeds 10,000 edits or 16 MiB of inserted text; narrow the selection or query. No text was changed");
                }
                edits.push(edit);
            }
            if !*all {
                if let Some(edit) = first {
                    return Ok(Outcome::Replaced(vec![edit]));
                }
            }
            if edits.is_empty() {
                bail!("No matches");
            }
            Ok(Outcome::Replaced(edits))
        }
    }
}

impl App {
    pub(crate) fn set_search(&mut self, query: String, case: bool, word: bool) {
        let mut search = Search::new(query, case, word);
        search.regex_mode = self.search.regex_mode;
        search.selection_only = self.search.selection_only;
        search.scope = self.search.scope;
        self.search = search;
    }
    pub(crate) fn document_search(&mut self, mode: Mode) -> Result<()> {
        if let Mode::Replace { text, .. } = &mode {
            anyhow::ensure!(
                text.len() <= REPLACEMENT_BYTES,
                "Replacement exceeds 16 MiB"
            );
        }
        let view = self.active_editor().context("Focus an editor first")?;
        let v = &self.views[&view];
        let d = &self.documents[&v.document];
        anyhow::ensure!(
            !matches!(mode, Mode::Find { .. } | Mode::Replace { .. })
                || !self.search.selection_only
                || self.search.scope.is_some_and(|s| s.0 == v.document),
            "Select a region in this document or turn off selection-only search"
        );
        let selected = v.anchor.map(|a| (a.min(v.cursor), a.max(v.cursor)));
        let request = Request {
            ticket: self
                .document_search_state
                .ticket
                .fetch_add(1, Ordering::Relaxed)
                + 1,
            view,
            doc: v.document,
            version: d.content_version(),
            format: d.format.clone(),
            rope: d.rope().clone(),
            search: if matches!(mode, Mode::Occurrences { .. }) {
                let (a, b) = selected.context("Select a word first")?;
                if b - a > INLINE_BYTES {
                    bail!("The selection is too large for an occurrence search");
                }
                Search::new(d.slice(a, b).into_owned(), true, false)
            } else {
                self.search.clone()
            },
            cursor: if matches!(mode, Mode::Find { backward: true }) {
                selected.map_or(v.cursor, |s| s.0)
            } else {
                v.cursor
            },
            selected,
            mode,
        };
        if !matches!(request.mode, Mode::Statistics | Mode::Format(_)) {
            request.search.rope_regex()?;
        }
        if d.len() <= INLINE_BYTES {
            let result =
                run(&request, &self.document_search_state.ticket).map_err(|e| format!("{e:#}"));
            self.apply_document_search(&request, result.map_err(anyhow::Error::msg)?)?;
        } else {
            if self.document_search_state.worker.is_none() {
                let output = self.services.reply_sender();
                let latest = self.document_search_state.ticket.clone();
                self.document_search_state.worker =
                    Some(LatestWorker::new(move |request: Request| {
                        let result = run(&request, &latest).map_err(|e| format!("{e:#}"));
                        if latest.load(Ordering::Relaxed) == request.ticket {
                            let _ = output.send(Reply::DocumentSearch(request, result));
                        }
                    }));
            }
            self.document_search_state
                .worker
                .as_ref()
                .unwrap()
                .submit(request);
            self.document_search_state.busy = true;
            self.status = "Working on document… · Escape cancels".into();
        }
        Ok(())
    }
    pub(crate) fn document_search_done(
        &mut self,
        request: Request,
        result: Result<Outcome, String>,
    ) {
        if request.ticket != self.document_search_state.ticket.load(Ordering::Relaxed) {
            return;
        }
        self.document_search_state.busy = false;
        if let Err(e) = result
            .map_err(anyhow::Error::msg)
            .and_then(|outcome| self.apply_document_search(&request, outcome))
        {
            self.status = format!("Search: {e:#}");
        }
    }
    fn apply_document_search(&mut self, request: &Request, outcome: Outcome) -> Result<()> {
        if self
            .documents
            .get(&request.doc)
            .is_none_or(|d| d.content_version() != request.version)
            || self
                .views
                .get(&request.view)
                .is_none_or(|v| v.document != request.doc)
        {
            bail!("Document changed while searching; try again");
        }
        match outcome {
            Outcome::Format(format) => {
                let doc = self.documents.get_mut(&request.doc).unwrap();
                anyhow::ensure!(!doc.read_only, "Document is read-only");
                anyhow::ensure!(
                    doc.format == request.format,
                    "Format changed during validation; try again"
                );
                doc.format = format;
                doc.generation += 1;
                self.status = format!("Saving as {}", doc.format.label());
            }
            Outcome::Statistics(text) => self.status = text,
            Outcome::Found(start, end, index, count) => {
                let v = self.views.get_mut(&request.view).unwrap();
                v.anchor = Some(start);
                v.cursor = end;
                v.manual_scroll = false;
                self.status = if count == 0 {
                    "Match found".into()
                } else {
                    format!("Match {index} of {count}")
                };
            }
            Outcome::Selected(selections, limited) => {
                let count = selections.len();
                self.set_selections(request.view, selections);
                self.status = format!(
                    "{count} occurrences selected{}",
                    if limited { " · limit reached" } else { "" }
                );
            }
            Outcome::Replaced(edits) => {
                let count = edits.len();
                self.replace_in_document(request.doc, &edits)?;
                self.status = format!("Replaced {}", crate::counted(count, "match", "matches"));
            }
        }
        Ok(())
    }
}
