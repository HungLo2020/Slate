mod accessibility;
mod actions;
pub mod cli;
pub mod commands;
pub mod config;
mod cursors;
pub mod dap;
pub mod desktop;
pub mod document;
mod editing;
pub mod events;
mod files;
mod folding;
pub mod format;
pub mod fsio;
pub mod git;
mod highlight;
pub mod ide;
pub mod instance;
pub mod languages;
pub mod layout;
mod lsp;
pub mod navigation;
pub mod outline;
mod panes;
pub mod paths;
pub mod picker;
pub mod preferences;
mod presentation;
pub mod process;
pub mod profiles;
pub mod project;
mod recovery_io;
mod render;
pub mod rpc;
mod saves;
pub mod search;
mod service_events;
mod services;
mod session;
mod smart;
mod snippets;
pub mod tasks;
pub mod terminal;
mod terminal_input;
pub mod text_format;
pub mod text_presentation;
pub mod theme;
pub mod tools;
pub mod trust;
pub mod workspace;
mod wrap;

use anyhow::{bail, Context, Result};
use document::{previous, Document};
use layout::{Axis, Handle, Node, Rect, View};
use preferences::{Preferences, StartupMode};
use search::{Prompt, Search};
use serde::{Deserialize, Serialize};
use services::{Entry, GitEntry, IoJob, Job, Reply, Services};
use std::time::{Duration, Instant};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
use terminal::{Cell, Screen, TerminalSession};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct EditorView {
    pub document: u64,
    pub cursor: usize,
    pub anchor: Option<usize>,
    pub top: usize,
    pub left: usize,
    pub rows: u16,
    pub cols: u16,
    pub manual_scroll: bool,
    /// With soft wrap, the first visible row within line `top`.
    #[serde(default)]
    pub top_row: usize,
    /// nano-style mark: cursor movement extends the selection.
    #[serde(default)]
    pub mark: bool,
    /// Preferred display column for vertical movement.
    #[serde(default, skip)]
    pub goal: Option<usize>,
    /// Carets beyond the primary one.
    #[serde(default, skip)]
    pub extra: Vec<cursors::Selection>,
    /// Folded regions, by the byte offset of their header line's start.
    #[serde(default)]
    pub folds: Vec<usize>,
    /// Snippet tab stops still to visit, the current one, and visited ones.
    #[serde(default, skip)]
    pub stops: Vec<Vec<(usize, usize)>>,
    #[serde(default, skip)]
    pub current_stop: Vec<(usize, usize)>,
    #[serde(default, skip)]
    pub visited: Vec<Vec<(usize, usize)>>,
}
#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct Key {
    pub key: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub shift: bool,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Command {
    ScrollTo {
        pane: u64,
        line: usize,
    },
    /// Scroll an editor horizontally by display columns.
    ScrollColumns {
        pane: u64,
        delta: i32,
    },
    OpenSettings,
    PathDialogStart,
    PathDialogFinish {
        paths: Vec<PathBuf>,
        #[serde(default)]
        overwrite: bool,
    },
    AccessibleSelect {
        pane: u64,
        start: usize,
        end: usize,
    },
    AccessibleScroll {
        pane: u64,
        offset: usize,
    },
    EditorOnly,
    ShowWorkspace,
    ToggleWorkspace,
    InputMethod {
        text: String,
        #[serde(default)]
        replace_start: i32,
        #[serde(default)]
        replace_length: usize,
    },
    InvokeAction {
        id: String,
        #[serde(default)]
        argument: String,
    },
    SetClipboard {
        text: String,
    },
    Search {
        query: String,
        #[serde(default)]
        case_sensitive: bool,
        #[serde(default)]
        whole_word: bool,
        #[serde(default)]
        backward: bool,
    },
    Replace {
        query: String,
        replacement: String,
        #[serde(default)]
        all: bool,
        #[serde(default)]
        case_sensitive: bool,
        #[serde(default)]
        whole_word: bool,
    },
    GoToLine {
        line: usize,
    },
    Indent {
        #[serde(default)]
        outdent: bool,
    },
    Prompt {
        kind: String,
    },
    UpdatePrompt {
        input: String,
        #[serde(default)]
        replacement: String,
        #[serde(default)]
        case_sensitive: bool,
        #[serde(default)]
        whole_word: bool,
    },
    SubmitPrompt {
        #[serde(default)]
        all: bool,
    },
    DismissPrompt,
    Theme {
        foreground: String,
        background: String,
        selection: String,
        accent: String,
        #[serde(default)]
        selection_foreground: String,
    },
    ReloadSettings,
    Configure {
        name: String,
        value: String,
    },
    Pointer {
        pane: u64,
        row: usize,
        col: usize,
        kind: String,
        #[serde(default)]
        button: u8,
        #[serde(default)]
        shift: bool,
        #[serde(default)]
        ctrl: bool,
        #[serde(default)]
        alt: bool,
    },
    Key {
        #[serde(flatten)]
        key: Key,
    },
    Paste {
        text: String,
    },
    Click {
        pane: u64,
        row: usize,
        col: usize,
        #[serde(default)]
        shift: bool,
    },
    Scroll {
        pane: u64,
        delta: i32,
    },
    Open {
        path: PathBuf,
    },
    Browse {
        path: PathBuf,
    },
    Save,
    SaveAs {
        path: PathBuf,
        #[serde(default)]
        overwrite: bool,
    },
    New,
    CloseDocument {
        #[serde(default)]
        force: bool,
    },
    CloseTab {
        pane: u64,
        view: u64,
        #[serde(default)]
        force: bool,
    },
    Undo,
    Redo,
    SelectAll,
    Copy,
    Cut,
    Focus {
        pane: u64,
    },
    FocusNext,
    SwitchTab {
        pane: u64,
        index: usize,
    },
    NextTab,
    NewTerminal,
    TerminateTerminal,
    AddView {
        kind: String,
    },
    Split {
        axis: Axis,
        #[serde(default)]
        kind: Option<String>,
    },
    ClosePane,
    MovePane {
        target: u64,
    },
    ResizeSplit {
        id: u64,
        ratio: f32,
    },
    Preset {
        name: String,
    },
    SaveLayout {
        name: String,
    },
    LoadLayout {
        name: String,
    },
    ToggleGit,
    Refresh,
    GitStage {
        path: String,
    },
    GitUnstage {
        path: String,
    },
    GitStageAll,
    GitUnstageAll,
    GitStageGroup {
        group: String,
    },
    GitDiff {
        path: String,
        #[serde(default)]
        staged: bool,
    },
    GitCommit {
        message: String,
    },
    Quit {
        #[serde(default)]
        force: bool,
    },
    /// Save every modified document; untitled ones ask for a path.
    SaveAll {
        #[serde(default)]
        quit: bool,
    },
    /// Open a picker: files, symbols, documents, search, diagnostics.
    OpenPicker {
        kind: String,
        #[serde(default)]
        query: String,
    },
    PickerQuery {
        query: String,
    },
    PickerMove {
        delta: i32,
    },
    /// Accept the selected item, or item `index` (counted over all items).
    PickerAccept {
        #[serde(default)]
        index: Option<usize>,
    },
    /// Toggle a search option: case, word or regex.
    PickerOption {
        name: String,
    },
    PickerClose,
    /// Send items from `first` on (the frontend scrolled the list).
    PickerWindow {
        first: usize,
    },
    /// Show hover text for the text under the mouse.
    HoverAt {
        pane: u64,
        row: usize,
        col: usize,
    },
    /// The mouse left: hide hover text.
    HoverClear,
    /// Accept a completion by its index in the list sent.
    CompletionAccept {
        index: usize,
    },
    /// A named editor or application action (see `actions.rs`).
    Action {
        name: String,
        #[serde(default)]
        argument: String,
    },
}
#[derive(Serialize)]
pub struct Tab {
    pub title: String,
    pub active: bool,
    pub editor_id: Option<u64>,
    pub close_id: Option<u64>,
}
#[derive(Serialize)]
pub struct PaneSnapshot {
    pub id: u64,
    pub rect: Rect,
    pub kind: String,
    pub tabs: Vec<Tab>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen: Option<std::sync::Arc<Screen>>,
    pub revision: u64,
    pub focused: bool,
    pub terminal_mouse_motion: bool,
    pub read_only: bool,
    pub selected: usize,
    pub rows: u16,
    pub cols: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<text_presentation::EditorText>,
    pub editor: Option<EditorPresentation>,
}
#[derive(Serialize)]
pub struct EditorPresentation {
    pub surrounding: String,
    pub cursor: usize,
    pub anchor: usize,
    pub selection: String,
    pub line_count: usize,
    pub top: usize,
    /// Horizontal scroll position in display columns.
    pub left: usize,
    pub document: u64,
    pub generation: u64,
    pub overview_revision: u64,
}
#[derive(Serialize)]
pub struct Snapshot {
    pub panes: Vec<PaneSnapshot>,
    pub handles: Vec<Handle>,
    pub focus: u64,
    pub files: Vec<Entry>,
    pub git: Vec<GitEntry>,
    pub browser: String,
    pub status: String,
    pub dirty: bool,
    pub quit: bool,
    pub clipboard: String,
    pub layouts: Vec<String>,
    pub prompt: Option<Prompt>,
    pub location: String,
    pub hints: String,
    pub background: String,
    pub foreground: String,
    pub accent: String,
    pub terminal_background: String,
    pub terminal_foreground: String,
    pub terminal_selection: String,
    /// Selection background, so monochrome terminals can show reverse video.
    pub selection: String,
    pub commands: Vec<commands::CommandInfo>,
    pub settings: Preferences,
    pub profiles: Vec<String>,
    pub setting_sources: BTreeMap<String, String>,
    pub editor_only: bool,
    /// Window title: the active document and the workspace folder.
    pub title: String,
    pub recent: Vec<String>,
    pub git_branch: String,
    pub git_repository: bool,
    pub git_busy: bool,
    pub git_error: String,
    pub files_revision: u64,
    pub git_revision: u64,
    pub picker: Option<picker::PickerView>,
    /// Restricted mode: project tools do not run.
    pub trusted: bool,
    /// The repository's folder is not trusted, so Git does not run.
    pub git_restricted: bool,
    pub completion: Option<lsp::CompletionView>,
    pub hover: Option<lsp::HoverView>,
    /// Errors and warnings in the workspace.
    pub problems: (usize, usize),
    /// The diagnostic under the caret.
    pub problem: String,
    /// What language servers are busy with.
    pub activity: String,
    pub debugging: bool,
    pub debug_paused: bool,
}
/// Saved layouts (`layouts.toml`).
#[derive(Serialize, Deserialize)]
struct Layouts {
    presets: BTreeMap<String, Node>,
}

/// `n` and a noun in the right number: "1 file", "3 files".
pub(crate) fn counted(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

pub struct App {
    pub root: PathBuf,
    pub documents: BTreeMap<u64, Document>,
    pub views: BTreeMap<u64, EditorView>,
    pub terminals: BTreeMap<u64, TerminalSession>,
    deferred_terminals: BTreeMap<u64, PathBuf>,
    pub editor_only: bool,
    pub layout: Node,
    pub focus: u64,
    pub status: String,
    pub quit: bool,
    pub clipboard: String,
    browser: PathBuf,
    files: Vec<Entry>,
    expanded_folders: std::collections::BTreeSet<PathBuf>,
    pending_file: Option<PathBuf>,
    pending_path_dialog: Option<(String, Option<u64>)>,
    /// The Git pane: entries, branch and running jobs.
    git: git::Panel,
    selected: usize,
    services: Services,
    ids: u64,
    presets: BTreeMap<String, Node>,
    /// Why `layouts.toml` could not be read; saving would overwrite it.
    layouts_unreadable: Option<String>,
    pub preferences: Preferences,
    preference_layers: profiles::Layers,
    pub prompt: Option<Prompt>,
    search: Search,
    pending_close: Option<(u64, u64)>,
    highlights: BTreeMap<u64, highlight::LineCache>,
    highlight_pending: BTreeMap<u64, (u64, bool)>,
    pending_open: BTreeMap<u64, u64>,
    saves: saves::SaveWorkflow,
    recovery: workspace::RecoveryState,
    colors: (String, String, String, String),
    terminal_colors: theme::Palette,
    system_colors: Option<(String, String, String, String, String)>,
    selection_foreground: String,
    events: events::Events,
    revision: u64,
    files_revision: u64,
    render_cache: BTreeMap<u64, (String, u64, std::sync::Arc<Screen>)>,
    pub screen_builds: u64,
    typing: bool,
    /// Launched with files (or standard input) rather than a directory.
    pub file_mode: bool,
    /// How this frontend performs privileged saves.
    pub elevation_mode: ElevationMode,
    /// Consecutive line cuts append to the cut buffer, as in nano:
    /// (view, document generation, cursor) after the last cut.
    last_cut: Option<(u64, u64, usize)>,
    misspellings: Vec<String>,
    /// The terminal frontend should suspend itself (SIGTSTP).
    pub suspend_requested: bool,
    /// The terminal interface hosts this session (enables suspend, mouse toggle).
    pub terminal_frontend: bool,
    /// Search matches by (document, generation, query).
    #[allow(clippy::type_complexity)]
    match_cache: Option<(u64, u64, String, std::sync::Arc<Vec<(usize, usize)>>)>,
    disk_check_at: Instant,
    disk_check_pending: bool,
    disk_conflicts: std::collections::VecDeque<(u64, Document)>,
    recent: Vec<String>,
    overview_cache: BTreeMap<u64, desktop::Overview>,
    overview_pending: BTreeMap<u64, (u64, usize)>,
    overview_workers: BTreeMap<u64, services::LatestWorker<desktop::OverviewRequest>>,
    overview_revision: u64,
    /// Requests only a frontend can fulfil (new window, print), drained by it.
    frontend_requests: Vec<String>,
    inbox: Option<instance::Inbox>,
    /// Cursor positions (line, column) for files whose open is in flight.
    pending_positions: BTreeMap<PathBuf, (usize, navigation::Column)>,
    /// Positions before jumps, for go-back and go-forward.
    back: Vec<(u64, usize)>,
    forward: Vec<(u64, usize)>,
    /// The open picker (files, symbols, search…).
    picker: Option<picker::Picker>,
    /// Workspace files for quick open, and when they were listed.
    index: std::sync::Arc<Vec<String>>,
    index_at: Option<Instant>,
    index_pending: bool,
    /// The current project search generation; older searches stop.
    search_cancel: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// The search that replace-in-files repeats, and the files it matched.
    last_search: Option<project::SearchOptions>,
    last_search_files: Vec<(PathBuf, Option<u64>)>,
    /// Files the last replace-in-files rewrote: (file, before, after).
    replace_backups: Vec<(PathBuf, Vec<u8>, Vec<u8>)>,
    /// Language servers, their documents and diagnostics.
    lsp: lsp::Lsp,
    /// Documents being formatted.
    formatting: BTreeMap<u64, format::Formatting>,
    /// Running tasks.
    tasks: BTreeMap<u64, tasks::Running>,
    /// The debug session and breakpoints.
    debug: dap::Debug,
    /// External tools from tools.toml.
    tools: Vec<tools::Tool>,
    /// Project tools (language servers, tasks, formatters, Git hooks) may run.
    trusted: bool,
    /// A command waiting for the user to trust the workspace.
    pending_trust: Option<(PathBuf, Command)>,
    /// Trust decisions for folders outside the workspace.
    trust_cache: std::sync::Mutex<std::collections::HashMap<PathBuf, bool>>,
    /// Wrapped rows are shared across cursor movement and rendering.
    #[allow(clippy::type_complexity)]
    wrap_cache:
        std::sync::Mutex<BTreeMap<(u64, u64, usize, usize, usize), std::sync::Arc<Vec<wrap::Row>>>>,
    fuzzy_worker: Option<picker::FilterWorker>,
    search_worker: Option<services::LatestWorker<picker::SearchRequest>>,
    /// Hidden line ranges per view, keyed by what they were computed from.
    #[allow(clippy::type_complexity)]
    fold_cache: std::sync::Mutex<BTreeMap<u64, ((u64, Vec<usize>, usize), folding::Hidden)>>,
}
/// A privileged write the terminal frontend performs with `sudo`, after
/// handing it the terminal for a password prompt.
pub struct Elevation {
    pub baseline: Option<fsio::Baseline>,
    pub document: u64,
    pub path: PathBuf,
    pub bytes: Vec<u8>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ElevationMode {
    /// No helper; permission errors are reported.
    #[default]
    None,
    /// The frontend takes `Elevation` requests and runs `sudo` interactively.
    Terminal,
    /// The IO worker runs this program (e.g. pkexec) without a terminal.
    Background(&'static str),
}
impl App {
    /// Whether this session keeps workspace recovery checkpoints.
    pub fn wants_recovery(&self) -> bool {
        !self.file_mode || self.preferences.file_recovery
    }
    pub fn events(&self) -> events::Events {
        self.events.clone()
    }
    pub fn revision(&self) -> u64 {
        self.revision.wrapping_add(self.events.generation())
    }
    pub fn process_events(&mut self) {
        self.events.drain();
        self.poll();
        self.drain_inbox();
        self.sync_document_preferences();
        self.reap_terminals();
        self.lsp_sync();
        self.expire_formatting();
        self.expire_lsp_requests();
        self.expire_tasks();
        self.debug_flush();
        self.watch_files();
        self.schedule_highlight();
        if self.recovery.dirty && self.recovery.at.elapsed() >= Duration::from_secs(1) {
            self.checkpoint();
        }
    }
    fn id(&mut self) -> u64 {
        self.ids += 1;
        self.ids
    }
    pub fn dirty(&self) -> bool {
        self.documents.values().any(Document::dirty)
    }
    fn focused(&self) -> Option<View> {
        self.layout.view(self.focus).cloned()
    }
    fn active_editor(&self) -> Option<u64> {
        match self.focused() {
            Some(View::Editor(id)) => Some(id),
            _ => None,
        }
    }
    fn editor_target(&mut self) -> u64 {
        if let Some(id) = self.active_editor() {
            return id;
        }
        for pane in self.layout.panes() {
            if let Some(View::Editor(id)) = self.layout.view(pane) {
                let id = *id;
                self.focus = pane;
                return id;
            }
        }
        let id = self.id();
        let doc = self.documents.keys().next().copied().unwrap_or_else(|| {
            let id = self.id();
            self.documents.insert(id, Document::scratch());
            id
        });
        self.views.insert(
            id,
            EditorView {
                document: doc,
                ..Default::default()
            },
        );
        self.add_tab(View::Editor(id));
        id
    }
    fn close_tab(&mut self, pane: u64, view: u64, force: bool) -> Result<()> {
        let index = self
            .layout
            .pane_mut(pane)
            .and_then(|(tabs, _)| {
                tabs.iter()
                    .position(|v| matches!(v, View::Editor(id) | View::Terminal(id) if *id == view))
            })
            .context("This tab is no longer open")?;
        if self.layout.pane_mut(pane).unwrap().0[index] == View::Terminal(view) {
            self.terminals.remove(&view);
            self.deferred_terminals.remove(&view);
            self.remove_terminal_views(view);
            self.status = format!("Closed Terminal {view}");
            return Ok(());
        }
        let doc = self
            .views
            .get(&view)
            .context("Missing editor view")?
            .document;
        if self.saves.pending_save.contains(&doc) {
            bail!("Wait for the pending save before closing this tab");
        }
        // Closed panes can leave cached views behind. Only tabs still present
        // in the layout count as another open view of this document.
        let mut shared = false;
        for pane in self.layout.panes() {
            for tab in self.layout.pane_mut(pane).unwrap().0.iter() {
                if let View::Editor(id) = tab {
                    shared |= *id != view && self.views.get(id).is_some_and(|v| v.document == doc);
                }
            }
        }
        let title = self.documents[&doc].title();
        if !shared && self.documents[&doc].dirty() && !force {
            self.pending_close = Some((pane, view));
            self.prompt = Some(Prompt {
                kind: "close-tab".into(),
                input: title,
                replacement: String::new(),
                field: 0,
                case_sensitive: false,
                whole_word: false,
            });
            return Ok(());
        }
        let (tabs, active) = self.layout.pane_mut(pane).unwrap();
        tabs.remove(index);
        if index < *active {
            *active -= 1;
        }
        *active = (*active).min(tabs.len().saturating_sub(1));
        if tabs.is_empty() {
            // Keep an editor available when its final file tab is closed.
            let scratch = self.id();
            let editor = self.id();
            self.documents.insert(scratch, Document::scratch());
            self.views.insert(
                editor,
                EditorView {
                    document: scratch,
                    ..Default::default()
                },
            );
            self.layout
                .pane_mut(pane)
                .unwrap()
                .0
                .push(View::Editor(editor));
        }
        self.views.remove(&view);
        if !shared {
            self.views.retain(|_, v| v.document != doc);
            self.documents.remove(&doc);
            self.highlights.remove(&doc);
            self.highlight_pending.remove(&doc);
        }
        if self.editor_only && !matches!(self.focused(), Some(View::Editor(_))) {
            self.expand_workspace()?;
        }
        self.status = format!("Closed {title}");
        Ok(())
    }
    /// Opening a file replaces an untouched, empty Untitled tab in the same
    /// pane, as desktop editors do.
    fn drop_pristine_scratch(&mut self, pane: u64, keep: u64) {
        let pristine: Vec<u64> = self
            .layout
            .tabs(pane)
            .unwrap_or_default()
            .iter()
            .filter_map(|t| match t {
                View::Editor(id) if *id != keep => Some(*id),
                _ => None,
            })
            .filter(|id| {
                let doc = self.views[id].document;
                let d = &self.documents[&doc];
                d.path.is_none()
                    && d.label.is_none()
                    && d.is_empty()
                    && !d.dirty()
                    && self.views.values().filter(|v| v.document == doc).count() == 1
            })
            .collect();
        for view in pristine {
            let doc = self.views[&view].document;
            if let Some((tabs, active)) = self.layout.pane_mut(pane) {
                if let Some(index) = tabs.iter().position(|t| *t == View::Editor(view)) {
                    tabs.remove(index);
                    if index < *active {
                        *active -= 1;
                    }
                    *active = (*active).min(tabs.len().saturating_sub(1));
                }
            }
            self.views.remove(&view);
            self.documents.remove(&doc);
            self.highlights.remove(&doc);
        }
    }
    /// Activate the tab `step` places after (or before) the current one.
    pub(crate) fn cycle_tab(&mut self, step: isize) -> Result<()> {
        let next = self.layout.pane_mut(self.focus).map(|(tabs, active)| {
            let len = tabs.len() as isize;
            let index = ((*active as isize + step).rem_euclid(len)) as usize;
            (index, !matches!(tabs[index], View::Editor(_)))
        });
        if let Some((index, tool)) = next {
            if self.editor_only && tool {
                self.expand_workspace()?;
            }
            *self.layout.pane_mut(self.focus).unwrap().1 = index;
        }
        Ok(())
    }
    fn add_tab(&mut self, view: View) {
        // Files and Git are singleton views within a pane. Reopening them
        // should reveal the existing tab instead of crowding the tab bar.
        if matches!(view, View::Files | View::Git) {
            if let Some((tabs, active)) = self.layout.pane_mut(self.focus) {
                if let Some(index) = tabs.iter().position(|tab| *tab == view) {
                    *active = index;
                    return;
                }
            }
        }
        self.show_view(view);
    }
    pub fn dispatch(&mut self, cmd: Command) {
        // Passive pointer motion has no editing effect. Only terminals requesting
        // all-motion reporting need it; desktop compositors can repeat hover events.
        if let Command::Pointer {
            pane, kind, shift, ..
        } = &cmd
        {
            if kind == "move"
                && (*shift
                    || !matches!(self.layout.view(*pane),
                Some(View::Terminal(id)) if self.terminals.get(id).is_some_and(TerminalSession::mouse_motion)))
            {
                return;
            }
        }
        self.sync_document_preferences();
        self.typing =
            matches!(&cmd, Command::Key { key } if !key.ctrl && !key.alt && !key.text.is_empty());
        if !self.typing {
            for doc in self.documents.values_mut() {
                doc.break_typing();
            }
        }
        self.revision += 1;
        if !matches!(cmd, Command::Theme { .. }) {
            self.recovery.dirty = true;
        }
        if let Err(e) = self.execute(cmd) {
            self.status = format!("Error: {e:#}");
        }
        self.typing = false;
        self.sync_document_preferences();
        self.schedule_highlight();
    }
    /// Open a prompt of `kind`, filled with the selection for find and replace.
    fn open_prompt(&mut self, kind: String) -> Result<()> {
        if ![
            "find",
            "replace",
            "goto",
            "open",
            "open-folder",
            "save-as",
            "commit",
            "layout-save",
            "layout-load",
            "move-pane",
            "settings",
            "insert-file",
            "set-encoding",
            "reopen-encoding",
            "set-line-ending",
            "open-recent",
            "new-file",
            "new-folder",
            "rename-file",
            "profile-save",
            "profile-load",
        ]
        .contains(&kind.as_str())
        {
            bail!("Unknown prompt");
        }
        let input = if ["find", "replace", "goto"].contains(&kind.as_str()) {
            let id = self.active_editor().context("Focus an editor first")?;
            let v = &self.views[&id];
            if kind == "goto" {
                String::new()
            } else {
                v.anchor
                    .map(|a| {
                        self.documents[&v.document]
                            .slice(a.min(v.cursor), a.max(v.cursor))
                            .to_string()
                    })
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| self.search.query.clone())
            }
        } else {
            String::new()
        };
        self.prompt = Some(Prompt {
            kind,
            input,
            replacement: String::new(),
            field: 0,
            case_sensitive: self.search.case_sensitive,
            whole_word: self.search.whole_word,
        });
        Ok(())
    }
    /// Act on the open prompt (`all`: the "all" choice, e.g. Replace All).
    fn submit_prompt(&mut self, all: bool) -> Result<()> {
        let p = self.prompt.clone().context("No active prompt")?;
        match p.kind.as_str() {
            // Graphical frontends answer confirmations through
            // dialogs: `all` selects the alternative button.
            "save-read-only" | "save-elevated" | "reload-changed" => {
                self.prompt = None;
                self.confirm_pending(&p.kind)?;
            }
            "file-changed" => self.resolve_disk_conflict(!all),
            "trust" => self.answer_trust(true)?,
            "project-replace" => {
                let files = self.last_search_files.len();
                self.prompt = Some(search::Prompt {
                    kind: "confirm-replace".into(),
                    input: format!("{files} file{}", if files == 1 { "" } else { "s" }),
                    replacement: p.input,
                    field: 0,
                    case_sensitive: false,
                    whole_word: false,
                });
            }
            "trash-file" => {
                self.prompt = None;
                self.confirm_trash()?;
            }
            "confirm-replace" => {
                self.prompt = None;
                self.replace_in_files(&p.replacement)?;
            }
            "quit" => {
                self.prompt = None;
                if all {
                    self.execute(Command::Quit { force: true })?;
                } else {
                    self.save_all(true)?;
                }
            }
            "close-tab" => {
                let target = self.pending_close.take();
                self.prompt = None;
                if all {
                    if let Some((pane, view)) = target {
                        self.close_tab(pane, view, true)?;
                    }
                } else if let Some((pane, view)) = target {
                    self.saves.close_after_save = Some((pane, view));
                    self.focus = pane;
                    self.reveal_document(self.views[&view].document);
                    self.execute(Command::Save)?;
                }
            }
            "open" | "open-folder" | "save-as" | "commit" | "layout-save" | "layout-load"
            | "move-pane" | "settings" => {
                if p.input.trim().is_empty() {
                    bail!("Enter {}", p.kind);
                }
                self.prompt = None;
                let line = format!(
                    "{} {}",
                    if p.kind == "settings" { "set" } else { &p.kind },
                    p.input
                );
                // A failure keeps the prompt open with what was typed.
                if let Err(e) = self
                    .resolve_command_input(&line)
                    .and_then(|cmd| self.execute(cmd))
                {
                    self.prompt = Some(p);
                    return Err(e);
                }
            }
            "insert-file" | "set-encoding" | "reopen-encoding" | "set-line-ending"
            | "open-recent" | "new-file" | "new-folder" | "rename-file" | "profile-save"
            | "profile-load" | "rename-symbol" | "debug-evaluate" | "debug-program" => {
                if p.input.trim().is_empty() {
                    bail!("Enter a value");
                }
                self.prompt = None;
                if let Err(e) = self.run_action(&p.kind, &p.input) {
                    self.prompt = Some(p);
                    return Err(e);
                }
            }
            "goto" => {
                self.execute(Command::GoToLine {
                    line: p.input.parse().context("Enter a line number")?,
                })?;
                self.prompt = None;
            }
            "replace" => self.execute(Command::Replace {
                query: p.input,
                replacement: p.replacement,
                all,
                case_sensitive: p.case_sensitive,
                whole_word: p.whole_word,
            })?,
            _ => self.execute(Command::Search {
                query: p.input,
                case_sensitive: p.case_sensitive,
                whole_word: p.whole_word,
                backward: false,
            })?,
        }
        Ok(())
    }
    /// A mouse event in a pane: terminals get it when they ask for mouse input.
    #[allow(clippy::too_many_arguments)]
    fn pointer(
        &mut self,
        pane: u64,
        row: usize,
        col: usize,
        kind: String,
        button: u8,
        shift: bool,
        ctrl: bool,
        alt: bool,
    ) -> Result<()> {
        if kind == "press" {
            self.focus = pane;
        }
        if let Some(View::Terminal(id)) = self.layout.view(pane).cloned() {
            self.terminals
                .get_mut(&id)
                .unwrap()
                .pointer(terminal::Pointer {
                    row,
                    col,
                    kind: &kind,
                    button,
                    shift,
                    ctrl,
                    alt,
                })?;
        } else if let (true, "press", Some(View::Editor(id))) =
            (alt, kind.as_str(), self.layout.view(pane).cloned())
        {
            // Alt+click adds (or removes) a caret.
            let offset = self.click_offset(id, row, col);
            self.toggle_cursor_at(id, offset);
        } else if kind == "press" || kind == "drag" {
            self.execute(Command::Click {
                pane,
                row,
                col,
                shift: shift || kind == "drag",
            })?;
        } else if kind == "double" || kind == "triple" {
            self.execute(Command::Click {
                pane,
                row,
                col,
                shift: false,
            })?;
            if let Some(View::Editor(id)) = self.layout.view(pane).cloned() {
                self.select_unit(id, kind == "triple");
            }
        }
        Ok(())
    }
    /// A click in a pane's text: moves the caret, or extends the selection with Shift.
    fn click(&mut self, pane: u64, row: usize, col: usize, shift: bool) -> Result<()> {
        self.focus = pane;
        match self.focused() {
            Some(View::Editor(id)) => {
                let v = &self.views[&id];
                if col < self.gutter_width(v.document, v.cols.max(1)) {
                    let shown = self.visible_rows(id, row + 1);
                    if let Some((line, 0, _)) = shown.get(row).copied() {
                        // The fold mark (▸ folded, ▾ foldable) toggles.
                        let doc = v.document;
                        let start = self.documents[&doc].line_offset(line);
                        if self.folded_header(id, line) {
                            self.views
                                .get_mut(&id)
                                .unwrap()
                                .folds
                                .retain(|f| *f != start);
                            return Ok(());
                        }
                        let gutter = self.gutter_width(doc, self.views[&id].cols.max(1));
                        let rope = self.documents[&doc].rope();
                        if col + 2 >= gutter
                            && smart::foldable(rope, line, self.preferences.indent_width)
                        {
                            self.views.get_mut(&id).unwrap().folds.push(start);
                            return Ok(());
                        }
                    }
                }
                let cursor = self.click_offset(id, row, col);
                let v = self.views.get_mut(&id).unwrap();
                v.manual_scroll = false;
                v.goal = None;
                v.extra.clear();
                v.stops.clear();
                if shift || v.mark {
                    v.anchor.get_or_insert(v.cursor);
                } else {
                    v.anchor = None;
                }
                v.cursor = cursor;
            }
            Some(View::Files) => self.selected = row.min(self.files.len().saturating_sub(1)),
            Some(View::Git) => {
                self.git.selected = row.min(self.git.entries.len().saturating_sub(1))
            }
            Some(View::Terminal(id)) => {
                self.terminals.get_mut(&id).unwrap().select(row, col, shift)
            }
            _ => {}
        }
        Ok(())
    }
    fn execute(&mut self, cmd: Command) -> Result<()> {
        match cmd {
            Command::EditorOnly => self.collapse_workspace(),
            Command::ShowWorkspace => self.expand_workspace()?,
            Command::ToggleWorkspace => {
                if self.editor_only {
                    self.expand_workspace()?;
                } else {
                    self.collapse_workspace();
                }
            }
            Command::ScrollTo { pane, line } => {
                if let Some(View::Editor(id)) = self.layout.view(pane) {
                    let v = self.views.get_mut(id).context("Missing editor view")?;
                    v.top = line.min(
                        self.documents[&v.document]
                            .line_count()
                            .saturating_sub(v.rows.max(1) as usize),
                    );
                    v.top_row = 0;
                    v.manual_scroll = true;
                }
            }
            Command::ScrollColumns { pane, delta } => {
                if let Some(View::Editor(id)) = self.layout.view(pane).cloned() {
                    self.scroll_columns(id, delta);
                }
            }
            Command::OpenSettings => {
                if !Preferences::path().exists() {
                    self.preferences.save()?;
                }
                self.execute(Command::Open {
                    path: Preferences::path(),
                })?;
            }
            Command::PathDialogStart => {
                let prompt = self.prompt.take().context("No path prompt")?;
                if !["open", "open-folder", "save-as"].contains(&prompt.kind.as_str()) {
                    bail!("Not a path prompt");
                }
                let doc = self.active_editor().map(|id| self.views[&id].document);
                self.pending_path_dialog = Some((prompt.kind, doc));
            }
            Command::PathDialogFinish { paths, overwrite } => {
                let (kind, doc) = self.pending_path_dialog.take().context("No path dialog")?;
                if paths.is_empty() {
                    if kind == "save-as" {
                        self.cancel_save_workflow();
                    }
                } else if kind == "save-as" {
                    let doc = doc.context("No document to save")?;
                    if !self.documents.contains_key(&doc) {
                        bail!("The document was closed");
                    }
                    self.reveal_document(doc);
                    self.execute(Command::SaveAs {
                        path: paths[0].clone(),
                        overwrite,
                    })?;
                } else {
                    if kind == "open-folder" {
                        self.execute(Command::ShowWorkspace)?;
                    }
                    for path in paths {
                        self.execute(Command::Open { path })?;
                    }
                }
            }
            Command::AccessibleSelect { pane, start, end } => {
                self.accessible_select(pane, start, end)?
            }
            Command::AccessibleScroll { pane, offset } => self.accessible_scroll(pane, offset)?,
            Command::InputMethod {
                text,
                replace_start,
                replace_length,
            } => {
                let id = self.active_editor().context("Focus an editor first")?;
                let text = text_format::normalize_input(&text).into_owned();
                if replace_start == 0 && replace_length == 0 {
                    self.edit(id, &text)?;
                } else {
                    let view = &self.views[&id];
                    let document = &self.documents[&view.document];
                    let cursor = document.utf16_offset(view.cursor);
                    let start = cursor
                        .checked_add_signed(replace_start as isize)
                        .context("Invalid input-method range")?;
                    let end = start
                        .checked_add(replace_length)
                        .context("Invalid input-method range")?;
                    let (start, end) = (
                        document
                            .byte_of_utf16(start)
                            .context("Invalid UTF-16 start")?,
                        document.byte_of_utf16(end).context("Invalid UTF-16 end")?,
                    );
                    self.edit_range(id, start, end, &text, view.cursor)?;
                }
            }
            Command::InvokeAction { id, argument } => self.invoke_action(&id, &argument)?,
            Command::SetClipboard { text } => self.clipboard = text,
            Command::Search {
                query,
                case_sensitive,
                whole_word,
                backward,
            } => {
                self.search = Search::new(query, case_sensitive, whole_word);
                self.find(backward)?;
            }
            Command::Replace {
                query,
                replacement,
                all,
                case_sensitive,
                whole_word,
            } => {
                self.search = Search::new(query, case_sensitive, whole_word);
                self.replace_matches(&replacement, all)?;
            }
            Command::GoToLine { line } => {
                let id = self.active_editor().context("Focus an editor first")?;
                let count = self.documents[&self.views[&id].document].line_count();
                if line == 0 || line > count {
                    bail!("Line must be between 1 and {count}");
                }
                let v = self.views.get_mut(&id).unwrap();
                v.cursor = self.documents[&v.document].line_offset(line - 1);
                v.anchor = None;
                v.manual_scroll = false;
            }
            Command::Indent { outdent } => self.indent(outdent)?,
            Command::Prompt { kind } => self.open_prompt(kind)?,
            Command::UpdatePrompt {
                input,
                replacement,
                case_sensitive,
                whole_word,
            } => {
                if let Some(p) = &mut self.prompt {
                    p.input = input;
                    p.replacement = replacement;
                    p.case_sensitive = case_sensitive;
                    p.whole_word = whole_word;
                }
            }
            Command::SubmitPrompt { all } => self.submit_prompt(all)?,
            Command::DismissPrompt => {
                let kind = self.prompt.take().map(|p| p.kind).unwrap_or_default();
                self.pending_close = None;
                if matches!(kind.as_str(), "rename-file" | "trash-file") {
                    self.pending_file = None;
                }
                if matches!(
                    kind.as_str(),
                    "save-as" | "save-read-only" | "save-elevated"
                ) {
                    self.cancel_save_workflow();
                }
                match kind.as_str() {
                    "trust" => self.answer_trust(false)?,
                    "save-read-only" | "save-elevated" | "reload-changed" => {
                        self.saves.pending_retry = None;
                        self.saves.quit_after_save = false;
                        self.status = "Save cancelled".into();
                    }
                    "quit" => self.status = "Quit cancelled".into(),
                    // Decide later: the next save reports the conflict.
                    "file-changed" => {
                        self.disk_conflicts.pop_front();
                        self.offer_disk_conflict();
                    }
                    _ => {}
                }
            }
            Command::Theme {
                foreground,
                background,
                selection,
                accent,
                selection_foreground,
            } => {
                let valid = |s: &str| {
                    s.len() == 7
                        && s.starts_with('#')
                        && s[1..].bytes().all(|b| b.is_ascii_hexdigit())
                };
                if ![&foreground, &background, &selection, &accent]
                    .iter()
                    .all(|s| valid(s))
                {
                    bail!("Invalid theme color");
                }
                let selection_foreground = if valid(&selection_foreground) {
                    selection_foreground
                } else {
                    self.selection_foreground.clone()
                };
                self.system_colors = Some((
                    foreground.clone(),
                    background.clone(),
                    selection.clone(),
                    accent.clone(),
                    selection_foreground.clone(),
                ));
                self.apply_pane_colors();
            }
            Command::ReloadSettings => {
                self.load_preferences();
                self.reload_tools();
                self.lsp_restart();
            }
            Command::Configure { name, value } => self.configure(&name, &value)?,
            Command::Pointer {
                pane,
                row,
                col,
                kind,
                button,
                shift,
                ctrl,
                alt,
            } => self.pointer(pane, row, col, kind, button, shift, ctrl, alt)?,
            Command::Key { key } => self.key(key)?,
            Command::OpenPicker { kind, query } => self.open_picker(&kind, &query)?,
            Command::PickerQuery { query } => self.picker_query(query)?,
            Command::PickerMove { delta } => self.picker_move(delta),
            Command::PickerAccept { index } => {
                if self.picker.as_ref().is_some_and(|p| p.kind == "search") {
                    self.remember_search();
                }
                self.picker_accept(index)?
            }
            Command::PickerOption { name } => self.picker_option(&name)?,
            Command::PickerClose => self.close_picker(),
            Command::PickerWindow { first } => self.picker_window(first),
            Command::CompletionAccept { index } => self.completion_select(index)?,
            Command::HoverAt { pane, row, col } => self.hover_at(pane, row, col),
            Command::HoverClear => {
                self.dismiss_hover();
            }
            Command::Paste { text } if self.picker.is_some() => {
                let query = format!(
                    "{}{}",
                    self.picker.as_ref().unwrap().query,
                    text.replace(['\n', '\r'], " ")
                );
                self.picker_query(query)?;
            }
            Command::Paste { text } if self.prompt.is_some() => {
                // Prompts hold a single line.
                let text = text_format::normalize_input(&text).replace('\n', " ");
                let p = self.prompt.as_mut().unwrap();
                if p.field == 0 {
                    p.input.push_str(&text);
                } else {
                    p.replacement.push_str(&text);
                }
            }
            Command::Paste { text } => match self.focused() {
                Some(View::Terminal(id)) => self
                    .terminals
                    .get_mut(&id)
                    .context("Terminal missing")?
                    .paste(&text)?,
                // Terminals paste carriage returns; documents store `\n` lines.
                Some(View::Editor(id)) if !self.views[&id].extra.is_empty() => {
                    self.multi_paste(id, &text_format::normalize_input(&text))?
                }
                Some(View::Editor(id)) => self.edit(id, &text_format::normalize_input(&text))?,
                _ => {}
            },
            Command::Click {
                pane,
                row,
                col,
                shift,
            } => self.click(pane, row, col, shift)?,
            Command::Scroll { pane, delta } => match self.layout.view(pane).cloned() {
                Some(View::Editor(id)) => self.scroll_view(id, delta),
                Some(View::Terminal(id)) => self.terminals.get_mut(&id).unwrap().scroll(delta),
                _ => {}
            },
            Command::Open { path } => {
                let path = if path.is_absolute() {
                    path
                } else {
                    self.root.join(path)
                };
                let token = self.id();
                self.services
                    .io
                    .send(IoJob::Open(token, path))
                    .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                self.pending_open.insert(token, self.focus);
                self.status = "Opening file…".into();
            }
            Command::Browse { path } => self.browse(path)?,
            Command::New => {
                let doc = self.id();
                self.documents.insert(doc, Document::scratch());
                let _ = self.editor_target();
                let view = self.id();
                self.views.insert(
                    view,
                    EditorView {
                        document: doc,
                        ..Default::default()
                    },
                );
                self.add_tab(View::Editor(view));
            }
            Command::Save | Command::SaveAs { .. } => {
                let (destination, overwrite) = if let Command::SaveAs { path, overwrite } = cmd {
                    (
                        Some(if path.is_absolute() {
                            path
                        } else {
                            self.root.join(path)
                        }),
                        overwrite,
                    )
                } else {
                    (None, false)
                };
                let id = self.active_editor().context("Focus an editor to save")?;
                let doc = self.views[&id].document;
                if destination.is_none() && self.documents[&doc].path.is_none() {
                    return self.execute(Command::Prompt {
                        kind: "save-as".into(),
                    });
                }
                if self.formatting.contains_key(&doc) {
                    bail!("Wait for formatting to finish");
                }
                if destination.is_none()
                    && self.preferences.format_on_save
                    && self.documents[&doc].dirty()
                    && self.format_document(doc, true)?
                {
                    return Ok(());
                }
                self.start_save(doc, destination, overwrite, false)?;
            }
            Command::SaveAll { quit } => self.save_all(quit)?,
            Command::Action { name, argument } => self.run_action(&name, &argument)?,
            Command::CloseDocument { force } => {
                let id = match self.focused() {
                    Some(View::Editor(id) | View::Terminal(id)) => id,
                    _ => bail!("Focus an editor or terminal first"),
                };
                self.close_tab(self.focus, id, force)?;
            }
            Command::CloseTab { pane, view, force } => self.close_tab(pane, view, force)?,
            Command::Undo | Command::Redo => {
                let id = self.active_editor().context("Focus an editor first")?;
                let view = self.views[&id].clone();
                let d = self.documents.get_mut(&view.document).unwrap();
                let changes = if matches!(cmd, Command::Undo) {
                    d.undo_changes()
                } else {
                    d.redo_changes()
                };
                let cursor = if matches!(cmd, Command::Undo) {
                    d.undo(view.cursor)
                } else {
                    d.redo(view.cursor)
                };
                if let Some(cursor) = cursor {
                    for (start, old, new) in changes {
                        self.rebase_views(view.document, start, start + old, new);
                    }
                    let v = self.views.get_mut(&id).unwrap();
                    v.cursor = cursor;
                    v.anchor = None;
                    v.extra.clear();
                }
            }
            Command::SelectAll => {
                if let Some(id) = self.active_editor() {
                    let v = self.views.get_mut(&id).unwrap();
                    v.anchor = Some(0);
                    v.cursor = self.documents[&v.document].len();
                }
            }
            Command::Copy | Command::Cut => {
                if let Some(View::Terminal(id)) = self.focused() {
                    self.clipboard = self.terminals[&id].selection_text();
                }
                if let Some(id) = self
                    .active_editor()
                    .filter(|id| !self.views[id].extra.is_empty())
                {
                    self.clipboard = self.selections_text(id);
                    if matches!(cmd, Command::Cut) {
                        let typed = self
                            .selections(id)
                            .iter()
                            .map(|s| {
                                let (a, b) = s.range();
                                smart::Typed::plain(a, b, "")
                            })
                            .collect();
                        self.apply_typed(id, typed)?;
                    }
                } else if let Some(id) = self.active_editor() {
                    let v = &self.views[&id];
                    if let Some(anchor) = v.anchor {
                        self.clipboard = self.documents[&v.document]
                            .slice(v.cursor.min(anchor), v.cursor.max(anchor))
                            .into_owned();
                        if matches!(cmd, Command::Cut) {
                            self.edit(id, "")?;
                        }
                    }
                }
            }
            Command::Focus { pane } => {
                if self.layout.view(pane).is_some() {
                    if self.editor_only && pane != self.focus {
                        self.expand_workspace()?;
                    }
                    self.focus = pane;
                }
            }
            Command::FocusNext if self.editor_only => {}
            Command::FocusNext => {
                let panes = self.layout.panes();
                let current = panes.iter().position(|p| *p == self.focus).unwrap_or(0);
                self.focus = panes[(current + 1) % panes.len()];
            }
            Command::SwitchTab { pane, index } => {
                let target = self
                    .layout
                    .pane_mut(pane)
                    .and_then(|(tabs, _)| tabs.get(index).cloned());
                if self.editor_only
                    && target.is_some_and(|v| pane != self.focus || !matches!(v, View::Editor(_)))
                {
                    self.expand_workspace()?;
                }
                if let Some((tabs, active)) = self.layout.pane_mut(pane) {
                    if index < tabs.len() {
                        *active = index;
                        self.focus = pane;
                    }
                }
            }
            Command::NextTab => self.cycle_tab(1)?,
            Command::NewTerminal => {
                self.expand_workspace()?;
                let id = self.new_terminal()?;
                self.add_tab(View::Terminal(id));
            }
            Command::TerminateTerminal => {
                let Some(View::Terminal(id)) = self.focused() else {
                    bail!("Focus a terminal first")
                };
                self.close_tab(self.focus, id, false)?;
            }
            Command::AddView { kind } => {
                if kind != "editor" {
                    self.expand_workspace()?;
                }
                let v = self.create_view(&kind)?;
                self.add_tab(v);
            }
            Command::Split { axis, kind } => {
                self.expand_workspace()?;
                if !self.can_split() {
                    bail!("This pane is nested too deeply to split again");
                }
                let v = if let Some(kind) = kind {
                    self.create_view(&kind)?
                } else {
                    match self.focused() {
                        Some(View::Editor(id)) => {
                            let new = self.id();
                            self.views.insert(new, self.views[&id].clone());
                            View::Editor(new)
                        }
                        Some(View::Terminal(_)) => View::Terminal(self.new_terminal()?),
                        Some(v) => v,
                        None => View::Files,
                    }
                };
                let pane = self.id();
                let split = self.id();
                self.layout.split(self.focus, axis, pane, split, v);
                self.focus = pane;
            }
            Command::ClosePane => {
                if self.editor_only {
                    bail!("Expand the workspace before closing a pane");
                }
                let Some(tabs) = self.layout.remove(self.focus) else {
                    bail!("Cannot close the final pane");
                };
                // Closing a pane never closes its documents or shells: its
                // tabs move to a remaining pane.
                self.focus = self.layout.panes()[0];
                self.adopt_orphans();
                let moved = tabs
                    .iter()
                    .filter(|v| matches!(v, View::Editor(_) | View::Terminal(_)))
                    .count();
                self.status = match moved {
                    0 => "Closed pane".into(),
                    1 => "Closed pane · moved 1 tab".into(),
                    n => format!("Closed pane · moved {n} tabs"),
                };
            }
            Command::MovePane { target } => {
                self.expand_workspace()?;
                self.layout.swap(self.focus, target);
            }
            Command::ResizeSplit { id, ratio } => {
                self.layout.resize(id, ratio);
            }
            Command::Preset { name } => self.preset(&name)?,
            Command::SaveLayout { name } => {
                if name.is_empty() || name.len() > 80 {
                    bail!("Layout name must be 1–80 characters");
                }
                self.presets.insert(name.clone(), self.layout.clone());
                self.write_layouts()?;
                self.status = format!("Saved layout {name}");
            }
            Command::LoadLayout { name } => {
                let node = self.presets.get(&name).context("Unknown layout")?.clone();
                self.restore_layout(node)?;
            }
            Command::ToggleGit => {
                self.expand_workspace()?;
                let v = if matches!(self.focused(), Some(View::Git)) {
                    View::Files
                } else {
                    View::Git
                };
                self.add_tab(v);
                self.refresh_git();
            }
            Command::Refresh => self.refresh(),
            cmd @ (Command::GitStage { .. }
            | Command::GitUnstage { .. }
            | Command::GitStageAll
            | Command::GitUnstageAll
            | Command::GitStageGroup { .. }
            | Command::GitDiff { .. }
            | Command::GitCommit { .. }) => self.git_command(cmd)?,
            Command::Quit { force } => {
                if self.dirty() && !force {
                    let count = self.documents.values().filter(|d| d.dirty()).count();
                    self.prompt = Some(Prompt {
                        kind: "quit".into(),
                        input: format!(
                            "{count} unsaved document{}",
                            if count == 1 { "" } else { "s" }
                        ),
                        replacement: String::new(),
                        field: 0,
                        case_sensitive: false,
                        whole_word: false,
                    });
                    return Ok(());
                }
                if !self.saves.pending_save.is_empty() {
                    bail!("Wait for pending file saves before quitting");
                }
                if force {
                    for d in self.documents.values_mut() {
                        d.discard_changes();
                    }
                    let docs = self.documents.keys().copied().collect::<Vec<_>>();
                    for doc in docs {
                        self.clamp_views(doc);
                    }
                }
                self.pending_open.clear();
                self.prompt = None;
                self.quit = true;
            }
        }
        Ok(())
    }
    fn browse(&mut self, path: PathBuf) -> Result<()> {
        let path = if path.is_absolute() {
            path
        } else {
            self.browser.join(path)
        };
        let path = files::workspace_directory(&path, &self.root)?;
        self.browser = path.clone();
        self.selected = 0;
        self.services.tx.send(Job::Browse(
            path,
            self.root.clone(),
            self.expanded_folders.iter().cloned().collect(),
            !self.terminal_frontend,
        ))?;
        Ok(())
    }
    pub fn refresh(&mut self) {
        let _ = self.services.tx.send(Job::Browse(
            self.browser.clone(),
            self.root.clone(),
            self.expanded_folders.iter().cloned().collect(),
            !self.terminal_frontend,
        ));
        self.refresh_git();
    }
    fn new_terminal(&mut self) -> Result<u64> {
        let id = self.id();
        let terminal = self.spawn_shell(&self.root.clone())?;
        self.terminals.insert(id, terminal);
        Ok(id)
    }
    fn create_view(&mut self, kind: &str) -> Result<View> {
        Ok(match kind {
            "files" => View::Files,
            "git" => View::Git,
            "terminal" => View::Terminal(self.new_terminal()?),
            "editor" => {
                let id = self.id();
                let doc = self
                    .views
                    .values()
                    .next()
                    .map(|v| v.document)
                    .context("Missing document")?;
                self.views.insert(
                    id,
                    EditorView {
                        document: doc,
                        ..Default::default()
                    },
                );
                View::Editor(id)
            }
            _ => bail!("Unknown view type"),
        })
    }
    fn remove_terminal_views(&mut self, id: u64) {
        for pane in self.layout.panes() {
            let (tabs, active) = self.layout.pane_mut(pane).unwrap();
            let removed_before_active = tabs
                .iter()
                .take(*active)
                .filter(|v| **v == View::Terminal(id))
                .count();
            tabs.retain(|v| *v != View::Terminal(id));
            *active = active.saturating_sub(removed_before_active);
            if tabs.is_empty() {
                tabs.push(View::Files);
            }
            *active = (*active).min(tabs.len() - 1);
        }
    }
    fn preset(&mut self, name: &str) -> Result<()> {
        if name == "minimal" {
            self.collapse_workspace();
            return Ok(());
        }
        if !["development", "bottom_terminal"].contains(&name) {
            bail!("Unknown preset");
        }
        let editor = self.editor_target();
        self.ensure_terminals()?;
        self.editor_only = false;
        let terminal = self.terminals.keys().next().copied();
        match name {
            "development" => {
                let term = if let Some(id) = terminal {
                    id
                } else {
                    self.new_terminal()?
                };
                self.layout = Node::default_layout(editor, term);
            }
            "bottom_terminal" => {
                let term = if let Some(id) = terminal {
                    id
                } else {
                    self.new_terminal()?
                };
                self.layout = Node::Split {
                    id: 4,
                    axis: Axis::Horizontal,
                    ratio: 0.2,
                    first: Box::new(Node::pane(1, View::Files)),
                    second: Box::new(Node::Split {
                        id: 5,
                        axis: Axis::Vertical,
                        ratio: 0.7,
                        first: Box::new(Node::pane(2, View::Editor(editor))),
                        second: Box::new(Node::pane(3, View::Terminal(term))),
                    }),
                };
            }
            _ => bail!("Unknown preset"),
        }
        self.adopt_orphans();
        self.focus = self
            .layout
            .panes()
            .into_iter()
            .find(|p| matches!(self.layout.view(*p), Some(View::Editor(_))))
            .unwrap();
        Ok(())
    }
    fn layouts_path() -> PathBuf {
        paths::config_dir().join("layouts.toml")
    }
    fn read_layouts(&mut self) {
        match config::read::<Layouts>(&Self::layouts_path()) {
            Ok(layouts) => {
                self.presets = layouts
                    .map(|l| l.presets)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|(_, n)| n.validate())
                    .collect();
            }
            Err(error) => {
                self.status = format!("Error in {error}");
                self.layouts_unreadable = Some(error);
            }
        }
    }
    fn write_layouts(&self) -> Result<()> {
        if let Some(error) = &self.layouts_unreadable {
            bail!("Not saving layouts over {error}");
        }
        let text = toml::to_string_pretty(&Layouts {
            presets: self.presets.clone(),
        })?;
        fsio::write_private(&Self::layouts_path(), text.as_bytes())
    }
    fn restore_layout(&mut self, mut node: Node) -> Result<()> {
        if !node.validate() {
            bail!("Invalid saved layout");
        }
        // Saved layouts persist shape and view kinds. The open editors and
        // shells fill its slots in order; extra slots get new views, and
        // anything left over is moved into the new layout.
        let current = self.layout.views();
        let editors: std::collections::VecDeque<u64> = current
            .iter()
            .filter_map(|v| match v {
                View::Editor(id) => Some(*id),
                _ => None,
            })
            .collect();
        let shells: std::collections::VecDeque<u64> = current
            .iter()
            .filter_map(|v| match v {
                View::Terminal(id) => Some(*id),
                _ => None,
            })
            .collect();
        let doc = self
            .active_editor()
            .or_else(|| editors.front().copied())
            .and_then(|id| self.views.get(&id))
            .or_else(|| self.views.values().next())
            .map(|v| v.document)
            .context("Missing document")?;
        struct Pool {
            editors: std::collections::VecDeque<u64>,
            shells: std::collections::VecDeque<u64>,
            doc: u64,
        }
        fn rebind(app: &mut App, n: &mut Node, pool: &mut Pool) -> Result<()> {
            match n {
                Node::Pane { id, tabs, .. } => {
                    *id = app.id();
                    for v in tabs {
                        match v {
                            View::Editor(id) => {
                                *id = match pool.editors.pop_front() {
                                    Some(existing) => existing,
                                    None => {
                                        let new = app.id();
                                        app.views.insert(
                                            new,
                                            EditorView {
                                                document: pool.doc,
                                                ..Default::default()
                                            },
                                        );
                                        new
                                    }
                                };
                            }
                            View::Terminal(id) => {
                                *id = match pool.shells.pop_front() {
                                    Some(existing) => existing,
                                    None => app.new_terminal()?,
                                };
                            }
                            _ => {}
                        }
                    }
                }
                Node::Split {
                    id, first, second, ..
                } => {
                    *id = app.id();
                    rebind(app, first, pool)?;
                    rebind(app, second, pool)?;
                }
            }
            Ok(())
        }
        let mut pool = Pool {
            editors,
            shells,
            doc,
        };
        rebind(self, &mut node, &mut pool)?;
        self.ensure_terminals()?;
        self.editor_only = false;
        self.layout = node;
        self.focus = self.layout.panes()[0];
        self.adopt_orphans();
        Ok(())
    }
    /// Keep views of a document within its text without resetting carets,
    /// folds or snippet fields (after edits that rebased them).
    fn clamp_views_soft(&mut self, doc: u64) {
        let Some(d) = self.documents.get(&doc) else {
            return;
        };
        let lines = d.line_count();
        for v in self.views.values_mut().filter(|v| v.document == doc) {
            v.cursor = d.floor_boundary(v.cursor.min(d.len()));
            v.anchor = v.anchor.map(|a| d.floor_boundary(a.min(d.len())));
            for s in &mut v.extra {
                s.cursor = d.floor_boundary(s.cursor.min(d.len()));
                s.anchor = s.anchor.map(|a| d.floor_boundary(a.min(d.len())));
            }
            v.folds.retain(|f| *f <= d.len());
            v.top = v.top.min(lines.saturating_sub(1));
        }
    }
    fn clamp_views(&mut self, doc: u64) {
        self.reset_breakpoints(doc);
        let d = &self.documents[&doc];
        let lines = d.line_count();
        for v in self.views.values_mut().filter(|v| v.document == doc) {
            v.cursor = d.floor_boundary(v.cursor);
            v.anchor = v.anchor.map(|a| d.floor_boundary(a));
            v.extra.clear();
            v.stops.clear();
            v.visited.clear();
            v.current_stop.clear();
            v.folds.retain(|f| *f <= d.len());
            v.top = v.top.min(lines.saturating_sub(1));
        }
    }
    fn edit(&mut self, id: u64, value: &str) -> Result<()> {
        let v = self.views[&id].clone();
        let anchor = v.anchor.unwrap_or(v.cursor);
        let start = v.cursor.min(anchor);
        let end = v.cursor.max(anchor);
        self.edit_range(id, start, end, value, v.cursor)
    }
    /// Apply several replacements to a document as one undoable edit and
    /// move every view (carets, folds, snippet fields) along with them.
    pub(crate) fn replace_in_document(
        &mut self,
        doc: u64,
        edits: &[(usize, usize, String)],
    ) -> Result<Vec<(usize, usize)>> {
        let cursor = self
            .views
            .values()
            .find(|v| v.document == doc)
            .map_or(0, |v| v.cursor);
        let placed = self
            .documents
            .get_mut(&doc)
            .context("The document was closed")?
            .replace_many(edits, cursor)?;
        // From the end, each range is still in the original coordinates.
        let mut order: Vec<usize> = (0..edits.len()).collect();
        order.sort_by_key(|i| std::cmp::Reverse((edits[*i].0, edits[*i].1)));
        for i in order {
            let (start, end, text) = &edits[i];
            self.rebase_views_text(doc, *start, *end, text);
        }
        self.clamp_views_soft(doc);
        Ok(placed)
    }
    /// Move everything that points into a document across an edit that
    /// replaced `start..end` with `length` bytes (already applied).
    fn rebase_views(&mut self, doc: u64, start: usize, end: usize, length: usize) {
        self.rebase_views_for(doc, start, end, length, None);
    }
    /// `rebase_views` for an edit whose inserted text is known.
    fn rebase_views_text(&mut self, doc: u64, start: usize, end: usize, text: &str) {
        self.rebase_views_for(doc, start, end, text.len(), Some(text));
    }
    fn rebase_views_for(
        &mut self,
        doc: u64,
        start: usize,
        end: usize,
        length: usize,
        inserted: Option<&str>,
    ) {
        let position = |p: usize| {
            if p <= start {
                p
            } else if p >= end {
                p - end + start + length
            } else {
                start + length
            }
        };
        let insertion = start == end;
        // Text inserted at a line start that contains a line break pushes
        // that line down: marks on it (folds, breakpoints) follow the line.
        let pushed_line = inserted.and_then(|text| text.rfind('\n').map(|i| start + i + 1));
        let line_mark = |p: usize| -> Option<usize> {
            if insertion && p == start {
                Some(pushed_line.unwrap_or(p))
            } else if start <= p && p < end {
                // The line's start was replaced: the mark goes with it.
                None
            } else {
                Some(position(p))
            }
        };
        for other in self.views.values_mut().filter(|o| o.document == doc) {
            other.cursor = position(other.cursor);
            other.anchor = other.anchor.map(position);
            for s in &mut other.extra {
                s.cursor = position(s.cursor);
                s.anchor = s.anchor.map(position);
            }
            // The field being typed in grows with text typed at its end;
            // a later field that starts where the text went moves right.
            for (a, b) in other.current_stop.iter_mut() {
                let grows = *b == start && *a <= start;
                *a = position(*a);
                *b = if grows { start + length } else { position(*b) };
            }
            for stop in other.stops.iter_mut().chain(other.visited.iter_mut()) {
                for (a, b) in stop.iter_mut() {
                    let follows = insertion && *a == start;
                    *b = if follows { *b + length } else { position(*b) };
                    *a = if follows { *a + length } else { position(*a) };
                }
            }
            other.folds = other.folds.iter().filter_map(|f| line_mark(*f)).collect();
        }
        for (history_doc, offset) in self.back.iter_mut().chain(self.forward.iter_mut()) {
            if *history_doc == doc {
                *offset = position(*offset);
            }
        }
        self.rebase_breakpoints(doc, |p| line_mark(p).unwrap_or(start));
    }
    /// Apply one smart replacement at the primary caret.
    fn apply_single(&mut self, id: u64, typed: smart::Typed) -> Result<()> {
        let before = self.views[&id].cursor;
        self.edit_range(id, typed.start, typed.end, &typed.text, before)?;
        let v = self.views.get_mut(&id).unwrap();
        v.cursor = typed.start + typed.caret;
        v.anchor = typed.anchor.map(|a| typed.start + a);
        Ok(())
    }
    fn edit_range(
        &mut self,
        id: u64,
        start: usize,
        end: usize,
        value: &str,
        before: usize,
    ) -> Result<()> {
        let doc = self.views[&id].document;
        let document = self.documents.get_mut(&doc).unwrap();
        if self.typing && start == end {
            document.replace_typing(start, end, value, before)?;
        } else {
            document.replace(start, end, value, before)?;
        }
        self.rebase_views_text(doc, start, end, value);
        let view = self.views.get_mut(&id).unwrap();
        view.cursor = start + value.len();
        view.anchor = None;
        view.mark = false;
        view.goal = None;
        view.manual_scroll = false;
        Ok(())
    }
    pub fn key_binding(&self, key: &Key) -> Option<&str> {
        if self.prompt.is_some() {
            return None;
        }
        let chord = key_chord(key);
        // Tools with keys.
        if let Some(tool) = self.tools.iter().find(|t| {
            !t.key.is_empty()
                && key_chord(&Key {
                    key: t.key.rsplit('+').next().unwrap_or_default().to_string(),
                    ctrl: t.key.contains("Ctrl+"),
                    alt: t.key.contains("Alt+"),
                    shift: t.key.contains("Shift+"),
                    ..Default::default()
                }) == chord
        }) {
            return Some(&tool.id);
        }
        // Debugger keys (F9–F11…) take over while a session runs.
        if self.debugging() {
            if let Some(action) = self.preferences.debug_keys.get(&chord) {
                return Some(action);
            }
        }
        self.preferences
            .global_keys
            .get(&chord)
            .or_else(|| match self.focused_kind() {
                "editor" => self.preferences.editor_keys.get(&chord),
                "terminal" => self.preferences.terminal_keys.get(&chord),
                // Workbench actions also work from the file and Git panes.
                _ => self
                    .preferences
                    .editor_keys
                    .get(&chord)
                    .filter(|action| WORKBENCH_ACTIONS.contains(&action.as_str())),
            })
            .map(String::as_str)
    }
    /// Single-key answers for confirmation prompts. Returns true when handled.
    fn choice_key(&mut self, k: &Key) -> Result<bool> {
        let Some(kind) = self.prompt.as_ref().map(|p| p.kind.clone()) else {
            return Ok(false);
        };
        let letter = if !k.ctrl && !k.alt {
            k.key.to_lowercase()
        } else {
            String::new()
        };
        let cancel = k.key == "Escape" || letter == "c";
        match kind.as_str() {
            "trash-file" => {
                if letter == "y" || letter == "d" {
                    self.execute(Command::SubmitPrompt { all: false })?;
                } else if cancel || letter == "n" || k.key == "Enter" {
                    self.execute(Command::DismissPrompt)?;
                }
            }
            "confirm-replace" => {
                if letter == "y" || letter == "r" {
                    let replacement = self.prompt.take().unwrap().replacement;
                    self.replace_in_files(&replacement)?;
                } else if letter == "n" || cancel || k.key == "Enter" {
                    self.prompt = None;
                    self.status = "Replace cancelled".into();
                }
            }
            "trust" => {
                if letter == "y" || letter == "t" {
                    self.answer_trust(true)?;
                } else if letter == "n" || cancel || k.key == "Enter" {
                    self.answer_trust(false)?;
                }
            }
            "close-tab" => match k.key.as_str() {
                "Escape" | "Enter" => self.execute(Command::DismissPrompt)?,
                _ if letter == "d" => self.execute(Command::SubmitPrompt { all: true })?,
                _ if letter == "s" || letter == "y" => {
                    self.execute(Command::SubmitPrompt { all: false })?
                }
                _ => {}
            },
            "quit" => {
                if letter == "y" || letter == "s" {
                    self.prompt = None;
                    self.save_all(true)?;
                } else if letter == "n" || letter == "d" {
                    self.prompt = None;
                    self.execute(Command::Quit { force: true })?;
                } else if cancel || k.key == "Enter" {
                    self.prompt = None;
                    self.status = "Quit cancelled".into();
                }
            }
            "file-changed" => {
                if letter == "r" {
                    self.resolve_disk_conflict(true);
                } else if letter == "k" {
                    self.resolve_disk_conflict(false);
                } else if cancel {
                    self.execute(Command::DismissPrompt)?;
                }
            }
            "save-read-only" | "save-elevated" | "reload-changed" => {
                if letter == "y" {
                    self.prompt = None;
                    self.confirm_pending(&kind)?;
                } else if letter == "n" || cancel || k.key == "Enter" {
                    self.prompt = None;
                    self.saves.pending_retry = None;
                    self.cancel_save_workflow();
                    self.status = "Save cancelled".into();
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
    fn key(&mut self, k: Key) -> Result<()> {
        if self.choice_key(&k)? {
            return Ok(());
        }
        if self.prompt.is_none() && self.picker_key(&k)? {
            return Ok(());
        }
        // A hover closes on the next key; Escape only closes it.
        if self.dismiss_hover() && k.key == "Escape" {
            return Ok(());
        }
        if self.prompt.is_none() && self.completion_key(&k)? {
            return Ok(());
        }
        if self.prompt.as_ref().is_some_and(|p| p.kind == "settings") {
            return self.settings_key(&k);
        }
        if let Some(p) = &mut self.prompt {
            match k.key.as_str() {
                _ if k.alt && k.key.eq_ignore_ascii_case("c") => {
                    p.case_sensitive = !p.case_sensitive
                }
                _ if k.alt && k.key.eq_ignore_ascii_case("w") => p.whole_word = !p.whole_word,
                "Escape" => self.prompt = None,
                "Enter" => self.execute(Command::SubmitPrompt { all: k.ctrl })?,
                "Tab" if p.kind == "replace" => p.field = 1 - p.field,
                "Backspace" => {
                    let value = if p.field == 0 {
                        &mut p.input
                    } else {
                        &mut p.replacement
                    };
                    let at = previous(value, value.len());
                    value.truncate(at);
                }
                _ if !k.ctrl && !k.alt => {
                    let text = k.text.replace(['\r', '\n'], "");
                    if p.field == 0 {
                        p.input.push_str(&text);
                    } else {
                        p.replacement.push_str(&text);
                    }
                }
                _ => {}
            }
            return Ok(());
        }
        let binding = self.key_binding(&k).map(str::to_owned);
        if let Some(command) = binding {
            self.command_line(&command);
            return Ok(());
        }
        if k.key == "F6" {
            return self.execute(Command::FocusNext);
        }
        if self.focused_kind() == "git" && !k.ctrl && !k.alt {
            match k.key.to_lowercase().as_str() {
                "s" => {
                    self.invoke_action("stage", "")?;
                    return Ok(());
                }
                "u" => {
                    self.invoke_action("unstage", "")?;
                    return Ok(());
                }
                "c" => {
                    self.invoke_action("commit", "")?;
                    return Ok(());
                }
                "r" => {
                    self.invoke_action("refresh", "")?;
                    return Ok(());
                }
                _ => {}
            }
        }
        match self.focused() {
            Some(View::Terminal(id)) => {
                let term = self.terminals.get_mut(&id).context("Missing terminal")?;
                let bytes = terminal_key(&k, term.application_cursor());
                if !bytes.is_empty() {
                    term.write(&bytes)?;
                }
            }
            Some(View::Files) | Some(View::Git) => {
                let git = matches!(self.focused(), Some(View::Git));
                let length = if git {
                    self.git.entries.len()
                } else {
                    self.files.len()
                };
                let selected = if git {
                    &mut self.git.selected
                } else {
                    &mut self.selected
                };
                match k.key.as_str() {
                    "Up" => *selected = selected.saturating_sub(1),
                    "Down" => *selected = (*selected + 1).min(length.saturating_sub(1)),
                    "Right" if !git => return self.file_action("expand-folder", ""),
                    "Left" if !git => return self.file_action("collapse-folder", ""),
                    "Home" => *selected = 0,
                    "End" => *selected = length.saturating_sub(1),
                    "Enter" => {
                        if git {
                            if self.git.entries.get(*selected).is_some() {
                                return self.invoke_action("diff", "");
                            }
                        } else if let Some(e) = self.files.get(*selected) {
                            return self.execute(Command::Open {
                                path: PathBuf::from(&e.path),
                            });
                        }
                    }
                    _ => {}
                }
            }
            Some(View::Editor(id)) => {
                let name = match k.key.as_str() {
                    "Left" if k.ctrl => Some("word-left"),
                    "Right" if k.ctrl => Some("word-right"),
                    "Left" => Some("move-left"),
                    "Right" => Some("move-right"),
                    "Up" => Some("move-up"),
                    "Down" => Some("move-down"),
                    "Home" if k.ctrl => Some("document-start"),
                    "End" if k.ctrl => Some("document-end"),
                    "Home" => Some("line-start"),
                    "End" => Some("line-end"),
                    "PageUp" => Some("page-up"),
                    "PageDown" => Some("page-down"),
                    _ => None,
                };
                let multi = !self.views[&id].extra.is_empty();
                if let Some(name) = name {
                    return if multi {
                        self.multi_move(id, name, k.shift)
                    } else {
                        self.move_cursor(id, name, k.shift)
                    };
                }
                if k.key == "Tab" && !k.ctrl && !k.alt && self.snippet_tab(id, k.shift)? {
                    return Ok(());
                }
                if k.key == "Escape" {
                    let v = self.views.get_mut(&id).unwrap();
                    v.stops.clear();
                    v.visited.clear();
                    v.current_stop.clear();
                }
                if multi && self.multi_key(id, &k)? {
                    return Ok(());
                }
                let v = self.views[&id].clone();
                let d = &self.documents[&v.document];
                let (_, col) = d.line_col(v.cursor, self.preferences.indent_width);
                match k.key.as_str() {
                    "Backspace" if k.ctrl && v.anchor.is_none() => {
                        let start = actions::document_word_boundary(d, v.cursor, false);
                        self.edit_range(id, start, v.cursor, "", v.cursor)?;
                    }
                    "Delete" if k.ctrl && v.anchor.is_none() => {
                        let end = actions::document_word_boundary(d, v.cursor, true);
                        self.edit_range(id, v.cursor, end, "", v.cursor)?;
                    }
                    "Backspace"
                        if v.anchor.is_none()
                            && self.preferences.auto_close_brackets
                            && smart::pair_around(d.rope(), v.cursor).is_some() =>
                    {
                        let (a, b) = smart::pair_around(d.rope(), v.cursor).unwrap();
                        self.edit_range(id, a, b, "", v.cursor)?;
                    }
                    "Backspace" => {
                        if v.anchor.is_none() && v.cursor > 0 {
                            let previous = d.previous_boundary(v.cursor);
                            self.views.get_mut(&id).unwrap().anchor = Some(previous);
                        }
                        if self.views[&id].anchor.is_some() {
                            self.edit(id, "")?;
                        }
                    }
                    "Delete" => {
                        if v.anchor.is_none() && v.cursor < d.len() {
                            let next = d.next_boundary(v.cursor);
                            self.views.get_mut(&id).unwrap().anchor = Some(next);
                        }
                        if self.views[&id].anchor.is_some() {
                            self.edit(id, "")?;
                        }
                    }
                    "Enter" => {
                        let anchor = v.anchor.unwrap_or(v.cursor);
                        let unit = self.indent_unit();
                        let typed = smart::enter(
                            d.rope(),
                            v.cursor.min(anchor),
                            v.cursor.max(anchor),
                            &unit,
                            self.preferences.auto_indent,
                        );
                        self.apply_single(id, typed)?;
                    }
                    "Tab" => {
                        if k.shift
                            || v.anchor.is_some_and(|a| {
                                d.slice(a.min(v.cursor), a.max(v.cursor)).contains('\n')
                            })
                        {
                            self.indent(k.shift)?;
                        } else {
                            let value = if self.preferences.insert_spaces {
                                " ".repeat(
                                    self.preferences.indent_width
                                        - col % self.preferences.indent_width,
                                )
                            } else {
                                "\t".into()
                            };
                            self.edit(id, &value)?;
                        }
                    }
                    "Escape" => {
                        let v = self.views.get_mut(&id).unwrap();
                        v.anchor = None;
                        v.mark = false;
                        v.extra.clear();
                    }
                    _ if !k.ctrl && !k.alt && !k.text.is_empty() => {
                        let typed = text_format::normalize_input(&k.text).into_owned();
                        let selection = cursors::Selection {
                            cursor: v.cursor,
                            anchor: v.anchor,
                        };
                        let (start, end) = selection.range();
                        let smart = self.typed_at(id, selection, &typed);
                        if smart == smart::Typed::plain(start, end, &typed) {
                            self.edit(id, &typed)?;
                        } else {
                            self.apply_single(id, smart)?;
                        }
                        self.after_typing(&typed);
                        if self.preferences.hard_wrap && !typed.contains('\n') {
                            self.hard_wrap(id)?;
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        Ok(())
    }
    pub fn focused_kind(&self) -> &str {
        match self.focused() {
            Some(View::Editor(_)) => "editor",
            Some(View::Terminal(_)) => "terminal",
            Some(View::Git) => "git",
            _ => "files",
        }
    }
    pub fn selected_path(&self) -> Option<String> {
        if self.focused_kind() == "git" {
            self.git
                .entries
                .get(self.git.selected)
                .map(|e| e.path.clone())
        } else {
            self.files.get(self.selected).map(|e| e.path.clone())
        }
    }
}

pub fn terminal_key(k: &Key, application_cursor: bool) -> Vec<u8> {
    let modifier = 1 + u8::from(k.shift) + 2 * u8::from(k.alt) + 4 * u8::from(k.ctrl);
    let arrow = match k.key.as_str() {
        "Up" => Some('A'),
        "Down" => Some('B'),
        "Right" => Some('C'),
        "Left" => Some('D'),
        "Home" => Some('H'),
        "End" => Some('F'),
        _ => None,
    };
    if let Some(arrow) = arrow {
        return if modifier > 1 {
            format!("\x1b[1;{modifier}{arrow}").into_bytes()
        } else {
            format!("\x1b{}{arrow}", if application_cursor { 'O' } else { '[' }).into_bytes()
        };
    }
    let special = match k.key.as_str() {
        "Enter" => Some("\r"),
        "Backspace" => Some("\x7f"),
        "Tab" => Some(if k.shift { "\x1b[Z" } else { "\t" }),
        "Escape" => Some("\x1b"),
        "Delete" => Some("\x1b[3~"),
        "Insert" => Some("\x1b[2~"),
        "PageUp" => Some("\x1b[5~"),
        "PageDown" => Some("\x1b[6~"),
        "F1" => Some("\x1bOP"),
        "F2" => Some("\x1bOQ"),
        "F3" => Some("\x1bOR"),
        "F4" => Some("\x1bOS"),
        "F5" => Some("\x1b[15~"),
        "F10" => Some("\x1b[21~"),
        "F11" => Some("\x1b[23~"),
        "F12" => Some("\x1b[24~"),
        _ => None,
    };
    if let Some(s) = special {
        return s.as_bytes().to_vec();
    }
    let text = if k.text.is_empty() {
        k.key.clone()
    } else {
        k.text.clone()
    };
    let mut bytes = if k.ctrl {
        let c = k.key.chars().next().unwrap_or(' ').to_ascii_uppercase();
        if ('@'..='_').contains(&c) {
            vec![c as u8 - 64]
        } else if c == ' ' {
            vec![0]
        } else {
            vec![]
        }
    } else {
        text.into_bytes()
    };
    if k.alt {
        bytes.insert(0, 0x1b);
    }
    bytes
}

impl App {
    pub fn command_line(&mut self, line: &str) {
        self.revision += 1;
        match self.resolve_command_input(line) {
            Ok(cmd) => self.dispatch(cmd),
            Err(error) => self.status = format!("Error: {error:#}"),
        }
    }
    /// Resolve a command without executing it, so frontends can apply their
    /// confirmation and clipboard policies after the shared parsing step.
    pub fn resolve_command_line(&self, line: &str) -> Option<Command> {
        let (verb, args) = line.trim().split_once(' ').unwrap_or((line.trim(), ""));
        let args = args.trim();
        let selected = || self.selected_path().unwrap_or_default();
        match verb {
            tool if tool.starts_with("tool:") => Some(Command::Action {
                name: "run-tool".into(),
                argument: tool["tool:".len()..].to_string(),
            }),
            "prompt-find" | "prompt-replace" | "prompt-goto" => Some(Command::Prompt {
                kind: verb.trim_start_matches("prompt-").into(),
            }),
            "find" => Some(Command::Search {
                query: args.into(),
                case_sensitive: false,
                whole_word: false,
                backward: false,
            }),
            "find-next" | "find-previous" => Some(Command::Search {
                query: self.search.query.clone(),
                case_sensitive: self.search.case_sensitive,
                whole_word: self.search.whole_word,
                backward: verb == "find-previous",
            }),
            "replace" | "replace-all" => {
                args.split_once(" => ")
                    .map(|(query, replacement)| Command::Replace {
                        query: query.into(),
                        replacement: replacement.into(),
                        all: verb == "replace-all",
                        case_sensitive: false,
                        whole_word: false,
                    })
            }
            "goto" => args.parse().ok().map(|line| Command::GoToLine { line }),
            "indent" | "outdent" => Some(Command::Indent {
                outdent: verb == "outdent",
            }),
            "settings" => Some(Command::Prompt {
                kind: "settings".into(),
            }),
            "settings-reload" => Some(Command::ReloadSettings),
            "editor-only" => Some(Command::EditorOnly),
            "workspace" => Some(Command::ShowWorkspace),
            "toggle-workspace" => Some(Command::ToggleWorkspace),
            "set" => args
                .split_once(' ')
                .map(|(name, value)| Command::Configure {
                    name: name.into(),
                    value: value.into(),
                }),
            "open" | "open-folder" | "save-as" if args.is_empty() => {
                Some(Command::Prompt { kind: verb.into() })
            }
            "open" | "open-folder" => Some(Command::Open { path: args.into() }),
            "save" => Some(Command::Save),
            "save-as" => Some(Command::SaveAs {
                path: args.into(),
                overwrite: false,
            }),
            "new" => Some(Command::New),
            "close" => Some(Command::CloseDocument { force: false }),
            "discard-document" => Some(Command::CloseDocument { force: true }),
            "quit" => Some(Command::Quit { force: false }),
            "discard-quit" => Some(Command::Quit { force: true }),
            "undo" => Some(Command::Undo),
            "redo" => Some(Command::Redo),
            "copy" => Some(Command::Copy),
            "cut" => Some(Command::Cut),
            "paste" => Some(Command::Paste {
                text: self.clipboard.clone(),
            }),
            "select-all" => Some(Command::SelectAll),
            "split-right" => Some(Command::Split {
                axis: Axis::Horizontal,
                kind: None,
            }),
            "split-down" => Some(Command::Split {
                axis: Axis::Vertical,
                kind: None,
            }),
            "split-terminal-right" => Some(Command::Split {
                axis: Axis::Horizontal,
                kind: Some("terminal".into()),
            }),
            "split-terminal-down" => Some(Command::Split {
                axis: Axis::Vertical,
                kind: Some("terminal".into()),
            }),
            "terminal" => Some(Command::NewTerminal),
            "terminate-terminal" => Some(Command::TerminateTerminal),
            "files" | "git" | "editor" => Some(Command::AddView { kind: verb.into() }),
            "close-pane" => Some(Command::ClosePane),
            "move-pane" => args.parse().ok().map(|target| Command::MovePane { target }),
            "next-pane" => Some(Command::FocusNext),
            "next-tab" => Some(Command::NextTab),
            "preset" => Some(Command::Preset { name: args.into() }),
            "layout-save" => Some(Command::SaveLayout { name: args.into() }),
            "layout-load" => Some(Command::LoadLayout { name: args.into() }),
            "resize" => {
                let mut words = args.split_whitespace();
                words
                    .next()
                    .and_then(|s| s.parse().ok())
                    .zip(words.next().and_then(|s| s.parse().ok()))
                    .map(|(id, ratio)| Command::ResizeSplit { id, ratio })
            }
            "refresh" => Some(Command::Refresh),
            "stage" => Some(Command::GitStage { path: selected() }),
            "unstage" => Some(Command::GitUnstage { path: selected() }),
            "stage-all" => Some(Command::GitStageAll),
            "unstage-all" => Some(Command::GitUnstageAll),
            "stage-group" => Some(Command::GitStageGroup { group: args.into() }),
            "diff" => Some(Command::GitDiff {
                path: selected(),
                staged: self
                    .git
                    .entries
                    .get(self.git.selected)
                    .is_some_and(|e| e.staged),
            }),
            "commit" => Some(Command::GitCommit {
                message: args.into(),
            }),
            "save-all" => Some(Command::SaveAll { quit: false }),
            "save-all-quit" => Some(Command::SaveAll { quit: true }),
            verb if ACTION_VERBS.contains(&verb) => Some(Command::Action {
                name: verb.into(),
                argument: args.into(),
            }),
            _ => None,
        }
    }
}

/// Command-line verbs handled by `actions.rs`.
const ACTION_VERBS: &[&str] = &[
    "profile-save",
    "profile-load",
    "new-file",
    "new-folder",
    "rename-file",
    "trash-file",
    "toggle-folder",
    "expand-folder",
    "collapse-folder",
    "move-left",
    "move-right",
    "move-up",
    "move-down",
    "word-left",
    "word-right",
    "line-start",
    "line-end",
    "page-up",
    "page-down",
    "document-start",
    "document-end",
    "mark",
    "cut-line",
    "copy-line",
    "uncut",
    "justify",
    "word-count",
    "location",
    "spell-check",
    "next-misspelling",
    "help",
    "insert-file",
    "reload",
    "reopen-encoding",
    "set-encoding",
    "set-line-ending",
    "toggle-bom",
    "toggle-read-only",
    "toggle-soft-wrap",
    "toggle-line-numbers",
    "toggle-mouse",
    "toggle-backup",
    "suspend",
    "toggle-whitespace",
    "toggle-minimap",
    "toggle-auto-reload",
    "zoom-in",
    "zoom-out",
    "zoom-reset",
    "open-recent",
    "clear-recent",
    "new-window",
    "print",
    "trust-workspace",
    "restrict-workspace",
    "request-trust",
    "add-cursor-above",
    "add-cursor-below",
    "add-next-occurrence",
    "select-all-occurrences",
    "go-to-bracket",
    "fold",
    "unfold",
    "toggle-fold",
    "fold-all",
    "unfold-all",
    "quick-open",
    "go-to-symbol",
    "search-in-files",
    "replace-in-files",
    "undo-replace-in-files",
    "open-documents",
    "problems",
    "go-back",
    "go-forward",
    "hover",
    "go-to-definition",
    "find-references",
    "rename-symbol",
    "code-actions",
    "apply-code-action",
    "format-document",
    "trigger-completion",
    "workspace-symbols",
    "next-problem",
    "previous-problem",
    "restart-language-servers",
    "run-task",
    "run-build-task",
    "stop-task",
    "debug-start",
    "debug-continue",
    "debug-step-over",
    "debug-step-into",
    "debug-step-out",
    "debug-pause",
    "debug-stop",
    "debug-evaluate",
    "toggle-breakpoint",
    "clear-breakpoints",
    "breakpoints",
    "debug-call-stack",
    "debug-frame",
    "previous-tab",
    "language-servers",
    "run-tool",
];
/// Editor-table actions that are not about the focused text, so their keys
/// also work while the file or Git pane has focus.
const WORKBENCH_ACTIONS: &[&str] = &[
    "quick-open",
    "search-in-files",
    "open-documents",
    "go-back",
    "go-forward",
    "run-build-task",
    "debug-start",
    "workspace-symbols",
    "problems",
];
pub fn key_chord(k: &Key) -> String {
    // Shift is implied by shifted punctuation such as `}` or `_`, and
    // terminals disagree about reporting it, so it is not part of the chord.
    let mut chars = k.key.chars();
    let punctuation =
        matches!((chars.next(), chars.next()), (Some(c), None) if !c.is_alphanumeric());
    format!(
        "{}{}{}{}",
        if k.ctrl { "Ctrl+" } else { "" },
        if k.alt { "Alt+" } else { "" },
        if k.shift && !punctuation {
            "Shift+"
        } else {
            ""
        },
        k.key.to_lowercase()
    )
}
impl App {
    fn finish_open(&mut self, pane: u64, mut d: Document) {
        let path = d.path.clone();
        let notice = d.notice.take();
        let doc = if let Some((id, _)) = self.documents.iter().find(|(_, old)| old.path == path) {
            *id
        } else {
            let id = self.id();
            self.documents.insert(id, d);
            id
        };
        if matches!(self.layout.view(pane), Some(View::Editor(_))) {
            self.focus = pane;
        } else {
            self.editor_target();
        }
        let existing=self.layout.pane_mut(self.focus).and_then(|(tabs,_)|tabs.iter().position(|v|matches!(v,View::Editor(id) if self.views.get(id).is_some_and(|v|v.document==doc))));
        if let Some(index) = existing {
            *self.layout.pane_mut(self.focus).unwrap().1 = index;
        } else {
            let id = self.id();
            self.views.insert(
                id,
                EditorView {
                    document: doc,
                    ..Default::default()
                },
            );
            self.add_tab(View::Editor(id));
            self.drop_pristine_scratch(self.focus, id);
        }
        let path = path.unwrap();
        if let Some((line, column)) = self.pending_positions.remove(&path) {
            if let Some(View::Editor(view)) = self.layout.view(self.focus).cloned() {
                let doc = self.views[&view].document;
                let cursor = self.offset_for(doc, line, column);
                let v = self.views.get_mut(&view).unwrap();
                v.cursor = cursor;
                v.anchor = None;
                v.manual_scroll = false;
            }
        }
        self.remember_recent(&path);
        self.status = format!(
            "Opened {}{}",
            path.display(),
            notice.map(|n| format!(" · {n}")).unwrap_or_default()
        );
        self.recovery.dirty = true;
    }
    fn location(&self) -> String {
        if let Some(id) = self.active_editor() {
            let v = &self.views[&id];
            let d = &self.documents[&v.document];
            let (line, col) = d.line_col(v.cursor, self.preferences.indent_width);
            let mut parts = vec![
                format!("Ln {}, Col {}", line + 1, col + 1),
                format!(
                    "{} {}",
                    if self.preferences.insert_spaces {
                        "Spaces"
                    } else {
                        "Tabs"
                    },
                    self.preferences.indent_width
                ),
                d.format.label(),
            ];
            if d.read_only || d.file_read_only {
                parts.push("Read-only".into());
            }
            if v.mark {
                parts.push("Mark".into());
            }
            if self.preferences.soft_wrap {
                parts.push("Wrap".into());
            }
            parts.join(" · ")
        } else {
            self.focused_kind().into()
        }
    }
    fn hints(&self) -> String {
        let kind = self.focused_kind();
        let lookup = |verb: &str| {
            let scoped = match kind {
                "editor" => Some(&self.preferences.editor_keys),
                "terminal" => Some(&self.preferences.terminal_keys),
                _ => None,
            };
            scoped
                .into_iter()
                .chain([&self.preferences.global_keys])
                .find_map(|map| map.iter().find(|(_, v)| v.as_str() == verb))
                .map(|(key, _)| key.clone())
        };
        if self.preferences.keymap == "nano" && kind == "editor" {
            // nano's two-line shortcut list, condensed to one line.
            let caret = |key: String| {
                let key = key.replace("Ctrl+", "^").replace("Alt+", "M-");
                match key.rsplit_once(['^', '-']) {
                    Some((prefix, letter)) if letter.len() == 1 => {
                        format!("{}{}", &key[..=prefix.len()], letter.to_uppercase())
                    }
                    _ => key,
                }
            };
            return [
                ("help", "Help"),
                ("save", "Write Out"),
                ("prompt-find", "Where Is"),
                ("cut-line", "Cut"),
                ("uncut", "Paste"),
                ("quit", "Exit"),
                ("insert-file", "Read File"),
                ("prompt-replace", "Replace"),
                ("justify", "Justify"),
                ("spell-check", "Spell"),
                ("prompt-goto", "Go To Line"),
                ("location", "Location"),
            ]
            .iter()
            .filter_map(|(verb, label)| lookup(verb).map(|key| format!("{} {label}", caret(key))))
            .collect::<Vec<_>>()
            .join("  ");
        }
        let verbs = match kind {
            "editor" => vec!["save", "prompt-find"],
            "terminal" => vec!["copy", "paste"],
            _ => vec!["next-pane", "next-tab"],
        };
        let mut hints = vec!["F1 commands".to_string(), "F6 pane".to_string()];
        if let Some(key) = lookup("toggle-workspace") {
            hints.push(format!(
                "{key} {}",
                if self.editor_only {
                    "expand"
                } else {
                    "collapse"
                }
            ));
        }
        for verb in verbs {
            if let Some(key) = lookup(verb) {
                hints.push(format!("{key} {}", verb.trim_start_matches("prompt-")));
            }
        }
        if let Some(key) = lookup("quit") {
            hints.push(format!("{key} quit"));
        }
        if kind == "git" {
            hints.push("Enter diff · S stage · U unstage · C commit · R refresh".into());
        }
        if kind == "terminal" {
            hints.push("Shift-drag selects · Shift-wheel scrollback".into());
        }
        hints.join(" · ")
    }
}
