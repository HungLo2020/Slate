//! Incremental, viewport-first syntax highlighting.
//!
//! A worker keeps syntect's parse/highlight state per document, checkpointed
//! every `STRIDE` lines. A request names the visible lines and the first line
//! edited since the previous request; the worker resumes from the nearest
//! valid checkpoint, so typing re-parses a few dozen lines rather than the
//! whole document. Newer requests for a document cancel older work.
//!
//! The core keeps line-relative spans per line. Edits shift that cache and
//! mark lines stale; stale colours stay on screen until fresh ones arrive.
use ropey::Rope;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{mpsc, Arc, Mutex, OnceLock},
};
use syntect::{
    highlighting::{FontStyle, HighlightState, Highlighter, RangedHighlightIterator, ThemeSet},
    parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet},
};

const STRIDE: usize = 32;
/// Lines highlighted beyond the visible ones, so scrolling shows colour.
pub const MARGIN: usize = 64;

/// A styled byte range within one line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    pub fg: u32,
    pub bold: bool,
    pub italic: bool,
}
pub type LineSpans = Arc<Vec<Span>>;

pub fn syntaxes() -> &'static SyntaxSet {
    static SYNTAX: OnceLock<SyntaxSet> = OnceLock::new();
    // bat's grammar collection: TOML, TypeScript, Dockerfile, Nix, … beyond
    // syntect's built-in set.
    SYNTAX.get_or_init(two_face::syntax::extra_newlines)
}
fn themes() -> &'static ThemeSet {
    static THEMES: OnceLock<ThemeSet> = OnceLock::new();
    THEMES.get_or_init(ThemeSet::load_defaults)
}

/// The grammar for a file name, or the first line (shebang, modelines).
pub fn syntax_for<'a>(path: Option<&Path>, first_line: &str) -> &'a SyntaxReference {
    let ss = syntaxes();
    path.and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .and_then(|name| {
            // Whole names first (Makefile, Dockerfile, .bashrc), then extensions.
            ss.find_syntax_by_extension(name)
                .or_else(|| ss.find_syntax_by_extension(name.rsplit('.').next().unwrap_or(name)))
        })
        .or_else(|| ss.find_syntax_by_first_line(first_line))
        .unwrap_or_else(|| ss.find_syntax_plain_text())
}

pub struct Request {
    pub doc: u64,
    pub generation: u64,
    pub rope: Rope,
    pub path: Option<PathBuf>,
    pub light: bool,
    pub first: usize,
    pub last: usize,
    /// The first line changed since the previous request for this document.
    pub dirty_from: usize,
}
pub struct Response {
    pub doc: u64,
    pub generation: u64,
    pub light: bool,
    pub first: usize,
    pub lines: Vec<LineSpans>,
}

struct DocState {
    syntax: String,
    light: bool,
    /// State before line `i * STRIDE`.
    checkpoints: Vec<(ParseState, HighlightState)>,
}

/// The highlight worker. `latest` holds the newest requested generation per
/// document; older work stops early when it is superseded.
pub fn worker(
    requests: mpsc::Receiver<Request>,
    latest: Arc<Mutex<HashMap<u64, u64>>>,
    mut send: impl FnMut(Response) -> bool,
) {
    let mut states: HashMap<u64, DocState> = HashMap::new();
    while let Ok(first) = requests.recv() {
        // Coalesce: only the newest request per document matters, but the
        // earliest dirty line among them must still be honoured.
        let mut queue: Vec<Request> = vec![first];
        while let Ok(more) = requests.try_recv() {
            queue.push(more);
        }
        let mut newest: HashMap<u64, Request> = HashMap::new();
        for request in queue {
            match newest.get_mut(&request.doc) {
                Some(existing) => {
                    let dirty = existing.dirty_from.min(request.dirty_from);
                    *existing = request;
                    existing.dirty_from = dirty;
                }
                None => {
                    newest.insert(request.doc, request);
                }
            }
        }
        for (_, request) in newest {
            if let Some(response) = highlight_request(&mut states, &request, &latest) {
                if !send(response) {
                    return;
                }
            }
        }
    }
}

fn line_text(rope: &Rope, line: usize) -> std::borrow::Cow<'_, str> {
    let slice = rope.line(line);
    match slice.as_str() {
        Some(s) => std::borrow::Cow::Borrowed(s),
        None => std::borrow::Cow::Owned(slice.to_string()),
    }
}

fn highlight_request(
    states: &mut HashMap<u64, DocState>,
    request: &Request,
    latest: &Mutex<HashMap<u64, u64>>,
) -> Option<Response> {
    let ss = syntaxes();
    let first_line = line_text(&request.rope, 0);
    let syntax = syntax_for(request.path.as_deref(), &first_line);
    let theme = &themes().themes[if request.light {
        "InspiredGitHub"
    } else {
        "base16-ocean.dark"
    }];
    let highlighter = Highlighter::new(theme);
    let state = states.entry(request.doc).or_insert_with(|| DocState {
        syntax: String::new(),
        light: request.light,
        checkpoints: vec![],
    });
    if state.syntax != syntax.name || state.light != request.light {
        state.syntax = syntax.name.clone();
        state.light = request.light;
        state.checkpoints.clear();
    }
    // Checkpoint i is the state before line i*STRIDE; it stays valid when the
    // first change is on that line or later.
    let keep = request.dirty_from / STRIDE + 1;
    state
        .checkpoints
        .truncate(keep.min(state.checkpoints.len()));
    if state.checkpoints.is_empty() {
        state.checkpoints.push((
            ParseState::new(syntax),
            HighlightState::new(&highlighter, ScopeStack::new()),
        ));
    }
    let total = request.rope.len_lines();
    let last = request.last.min(total);
    let first = request.first.min(last);
    let start_checkpoint = (first / STRIDE).min(state.checkpoints.len() - 1);
    let (mut parse, mut highlight) = state.checkpoints[start_checkpoint].clone();
    let mut lines = Vec::with_capacity(last - first);
    for line in start_checkpoint * STRIDE..last {
        if line % STRIDE == 0 && line / STRIDE == state.checkpoints.len() {
            state.checkpoints.push((parse.clone(), highlight.clone()));
        }
        // Long catch-up parses stop when a newer request has arrived.
        if line % 256 == 0
            && latest
                .lock()
                .unwrap()
                .get(&request.doc)
                .is_some_and(|g| *g > request.generation)
        {
            return None;
        }
        // Minified/generated lines must not monopolize the syntax worker.
        if request.rope.line(line).len_bytes() > 64 * 1024 {
            if line >= first {
                lines.push(Arc::new(Vec::new()));
            }
            continue;
        }
        let text = line_text(&request.rope, line);
        let Ok(ops) = parse.parse_line(&text, ss) else {
            // A grammar failure leaves the rest of the document plain.
            lines.extend((line.max(first)..last).map(|_| Arc::new(vec![])));
            break;
        };
        if line < first {
            // Advance the highlighter state without collecting spans.
            for _ in RangedHighlightIterator::new(&mut highlight, &ops, &text, &highlighter) {}
            continue;
        }
        let content = text.trim_end_matches('\n').len();
        let mut spans = vec![];
        for (style, _, range) in
            RangedHighlightIterator::new(&mut highlight, &ops, &text, &highlighter)
        {
            let end = range.end.min(content);
            if range.start >= end {
                continue;
            }
            spans.push(Span {
                start: range.start as u32,
                end: end as u32,
                fg: (style.foreground.r as u32) << 16
                    | (style.foreground.g as u32) << 8
                    | style.foreground.b as u32,
                bold: style.font_style.contains(FontStyle::BOLD),
                italic: style.font_style.contains(FontStyle::ITALIC),
            });
        }
        lines.push(Arc::new(spans));
    }
    Some(Response {
        doc: request.doc,
        generation: request.generation,
        light: request.light,
        first,
        lines,
    })
}

/// Highlight a whole text at once (tests).
#[cfg(test)]
pub fn highlight_text(text: &str, path: Option<&Path>, light: bool) -> Vec<Vec<Span>> {
    let rope = Rope::from(text);
    let request = Request {
        doc: 0,
        generation: 0,
        last: rope.len_lines(),
        rope,
        path: path.map(Path::to_path_buf),
        light,
        first: 0,
        dirty_from: 0,
    };
    let mut states = HashMap::new();
    highlight_request(&mut states, &request, &Mutex::new(HashMap::new()))
        .map(|r| r.lines.iter().map(|l| l.to_vec()).collect())
        .unwrap_or_default()
}

/// The core's per-document line cache: spans and whether they are current.
#[derive(Default)]
pub struct LineCache {
    pub light: bool,
    /// Changes whenever spans change, for render caches.
    pub revision: u64,
    pub lines: Vec<Option<(LineSpans, bool)>>,
    /// Edits since the last complete response, for mapping late responses:
    /// (generation after the edit, first line, old line count, new line count).
    pub edits: Vec<(u64, usize, usize, usize)>,
    pub dirty_from: usize,
}
impl LineCache {
    pub fn new(lines: usize, light: bool) -> Self {
        Self {
            light,
            revision: 0,
            lines: vec![None; lines],
            edits: vec![],
            dirty_from: 0,
        }
    }
    /// Shift the cache for an edit replacing `old` lines from `first` (the
    /// first included) with `new` lines. Touched lines keep their old spans
    /// as stale ones.
    pub fn edit(&mut self, generation: u64, first: usize, old: usize, new: usize) {
        let first = first.min(self.lines.len());
        let end = (first + old).min(self.lines.len());
        let mut replacement: Vec<_> = self.lines[first..end]
            .iter()
            .take(new)
            .map(|entry| entry.as_ref().map(|(spans, _)| (spans.clone(), false)))
            .collect();
        replacement.resize(new, None);
        self.lines.splice(first..end, replacement);
        // A change can alter the state of everything below (e.g. an
        // unclosed comment): later lines stay visible but become stale.
        for entry in self.lines.iter_mut().skip(first + new).flatten() {
            entry.1 = false;
        }
        self.dirty_from = self.dirty_from.min(first);
        self.revision += 1;
        self.edits.push((generation, first, old, new));
        if self.edits.len() > 256 {
            self.edits.drain(..128);
        }
    }
    /// Store a response computed for `generation`, mapping its lines through
    /// edits made since. Lines inside later edits are skipped.
    pub fn apply(&mut self, generation: u64, current: u64, first: usize, lines: Vec<LineSpans>) {
        let later: Vec<_> = self
            .edits
            .iter()
            .filter(|(g, ..)| *g > generation)
            .copied()
            .collect();
        let fresh = generation == current && later.is_empty();
        self.revision += 1;
        'lines: for (index, spans) in lines.into_iter().enumerate() {
            let mut line = first + index;
            for (_, edit_first, old, new) in &later {
                if line < *edit_first {
                    continue;
                }
                if line < edit_first + old {
                    continue 'lines;
                }
                line = line + new - old;
            }
            if let Some(entry) = self.lines.get_mut(line) {
                *entry = Some((spans, fresh));
            }
        }
        if fresh {
            self.edits.clear();
        }
    }
    /// Whether every line in a range has current spans.
    pub fn current(&self, first: usize, last: usize) -> bool {
        self.first_missing(first, last).is_none()
    }
    /// The first line in a range without current spans.
    pub fn first_missing(&self, first: usize, last: usize) -> Option<usize> {
        (first..last.min(self.lines.len()))
            .find(|line| !matches!(self.lines.get(*line), Some(Some((_, true)))))
    }
}

use crate::{layout::View, App};

impl App {
    /// Ask for spans of visible lines that lack current ones.
    pub(super) fn schedule_highlight(&mut self) {
        let light = self.light_theme();
        let mut visible: Vec<(u64, usize, usize)> = vec![];
        for pane in self.layout.panes() {
            if let Some(View::Editor(id)) = self.layout.view(pane) {
                let v = &self.views[id];
                let rows = v.rows.max(24) as usize * 2;
                visible.push((
                    v.document,
                    v.top.saturating_sub(MARGIN),
                    v.top + rows + MARGIN,
                ));
            }
        }
        visible.sort_unstable();
        visible.dedup_by_key(|(doc, ..)| *doc);
        for (doc, first, last) in visible {
            let Some(d) = self.documents.get_mut(&doc) else {
                continue;
            };
            let edits = d.take_line_edits();
            let lines = d.line_count();
            let generation = d.generation;
            let cache = self
                .highlights
                .entry(doc)
                .or_insert_with(|| LineCache::new(lines, light));
            match edits {
                Some(edits) => {
                    for (first, old, new) in edits {
                        cache.edit(generation, first, old, new);
                    }
                }
                None => *cache = LineCache::new(lines, light),
            }
            if cache.light != light || cache.lines.len() != lines {
                *cache = LineCache::new(lines, light);
            }
            let Some(missing) = cache.first_missing(first, last) else {
                continue;
            };
            if self
                .highlight_pending
                .get(&doc)
                .is_some_and(|(g, l)| *g == generation && *l == light)
            {
                continue;
            }
            let dirty_from = cache.dirty_from;
            cache.dirty_from = usize::MAX;
            self.services
                .highlight_latest
                .lock()
                .unwrap()
                .insert(doc, generation);
            let request = Request {
                doc,
                generation,
                rope: self.documents[&doc].rope().clone(),
                path: self.documents[&doc].path.clone(),
                light,
                first: missing,
                last,
                dirty_from,
            };
            if self.services.highlight.try_send(request).is_ok() {
                self.highlight_pending.insert(doc, (generation, light));
            } else if let Some(cache) = self.highlights.get_mut(&doc) {
                // The worker is busy; try again on the next cycle.
                cache.dirty_from = cache.dirty_from.min(dirty_from);
            }
        }
    }
    pub(super) fn highlighted(&mut self, response: Response) {
        let Some(d) = self.documents.get(&response.doc) else {
            return;
        };
        if self
            .highlight_pending
            .get(&response.doc)
            .is_some_and(|(g, _)| *g <= response.generation)
        {
            self.highlight_pending.remove(&response.doc);
        }
        let current = d.generation;
        if response.light != self.light_theme() {
            return;
        }
        if let Some(cache) = self.highlights.get_mut(&response.doc) {
            cache.apply(response.generation, current, response.first, response.lines);
        }
    }
    /// Spans for a line (fresh or stale) and whether they are current.
    pub(crate) fn line_spans(&self, doc: u64, line: usize) -> Option<(LineSpans, bool)> {
        self.highlights.get(&doc)?.lines.get(line)?.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extended_grammars_cover_common_project_files() {
        for (name, text) in [
            ("Cargo.toml", "[package]\nname = \"x\"\n"),
            ("app.ts", "const x: number = 1;\n"),
            ("Dockerfile", "FROM alpine\n"),
            ("main.rs", "fn main() {}\n"),
        ] {
            let lines = highlight_text(text, Some(Path::new(name)), false);
            let colours: std::collections::BTreeSet<_> =
                lines.iter().flatten().map(|s| s.fg).collect();
            assert!(colours.len() > 1, "{name} was not highlighted");
        }
    }

    #[test]
    fn edits_shift_the_cache_and_late_responses_map_through_them() {
        let spans = |n: u32| {
            Arc::new(vec![Span {
                start: 0,
                end: n,
                fg: n,
                bold: false,
                italic: false,
            }])
        };
        let mut cache = LineCache::new(5, false);
        cache.apply(1, 1, 0, (0..5).map(|i| spans(i + 1)).collect());
        // Sending a request resets the dirty marker.
        cache.dirty_from = usize::MAX;
        assert!(cache.lines.iter().all(|l| l.as_ref().unwrap().1));
        // Line 1 becomes three lines.
        cache.edit(2, 1, 1, 3);
        assert_eq!(cache.lines.len(), 7);
        assert!(
            cache.lines[0].as_ref().unwrap().1,
            "lines above stay current"
        );
        assert!(
            !cache.lines[1].as_ref().unwrap().1,
            "the edited line is stale but kept"
        );
        assert!(cache.lines[2].is_none());
        assert_eq!(
            cache.lines[4].as_ref().unwrap().0[0].fg,
            3,
            "later lines shift"
        );
        assert!(!cache.lines[4].as_ref().unwrap().1);
        assert_eq!(cache.dirty_from, 1);
        // A response for generation 1 arriving after the edit: old line 3 maps
        // to line 5, the edited line 1 is skipped, and nothing is current.
        cache.apply(1, 2, 0, (0..5).map(|i| spans(10 + i)).collect());
        assert_eq!(cache.lines[5].as_ref().unwrap().0[0].fg, 13);
        assert!(!cache.lines[5].as_ref().unwrap().1);
        assert_eq!(cache.first_missing(0, 7), Some(0));
        cache.apply(2, 2, 0, (0..7).map(|i| spans(20 + i)).collect());
        assert_eq!(cache.first_missing(0, 7), None);
    }

    #[test]
    fn resuming_from_checkpoints_matches_a_full_parse() {
        let mut text = String::new();
        for i in 0..200 {
            text.push_str(&format!("fn f{i}() {{ let s = \"x\"; }} // c\n"));
        }
        let rope = Rope::from(text.as_str());
        let path = Some(PathBuf::from("a.rs"));
        let mut states = HashMap::new();
        let latest = Mutex::new(HashMap::new());
        let request = |generation, first, last, dirty_from| Request {
            doc: 1,
            generation,
            rope: rope.clone(),
            path: path.clone(),
            light: false,
            first,
            last,
            dirty_from,
        };
        let full = highlight_request(&mut states, &request(1, 0, 200, 0), &latest).unwrap();
        let partial =
            highlight_request(&mut states, &request(2, 150, 160, usize::MAX), &latest).unwrap();
        assert_eq!(partial.lines, full.lines[150..160].to_vec());
        assert_eq!(states[&1].checkpoints.len(), 200 / STRIDE + 1);
        // A superseded request stops early.
        latest.lock().unwrap().insert(1, 9);
        assert!(highlight_request(&mut states, &request(3, 0, 200, 0), &latest).is_none());
    }
}
