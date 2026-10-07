mod actions;
pub mod cli;
pub mod commands;
pub mod desktop;
pub mod document;
mod editing;
pub mod events;
pub mod fsio;
mod highlight;
pub mod instance;
pub mod layout;
pub mod paths;
pub mod preferences;
mod presentation;
pub mod search;
mod services;
pub mod terminal;
pub mod text_format;
pub mod text_presentation;
pub mod workspace;
mod wrap;

use anyhow::{bail, Context, Result};
use document::{line_start, next, previous, Document};
use layout::{Axis, Handle, Node, Rect, View};
use preferences::{Preferences, StartupMode};
use search::{Prompt, Search};
use serde::{Deserialize, Serialize};
use services::{Entry, GitEntry, IoJob, Job, Reply, Services};
use std::time::{Duration, Instant};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
use terminal::{Cell, Screen, TerminalSession};
use unicode_segmentation::UnicodeSegmentation;

use workspace::WorkspaceStore;

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
#[derive(Debug, Deserialize)]
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
    /// Selection background, so monochrome terminals can show reverse video.
    pub selection: String,
    pub commands: Vec<commands::CommandInfo>,
    pub settings: Preferences,
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
}
#[derive(Serialize, Deserialize)]
struct Settings {
    presets: BTreeMap<String, Node>,
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
    git: Vec<GitEntry>,
    selected: usize,
    git_selected: usize,
    services: Services,
    ids: u64,
    presets: BTreeMap<String, Node>,
    pub preferences: Preferences,
    pub prompt: Option<Prompt>,
    search: Search,
    pending_close: Option<(u64, u64)>,
    tokens: BTreeMap<u64, (u64, bool, Vec<highlight::Token>)>,
    highlight_pending: BTreeMap<u64, (u64, bool)>,
    pending_open: BTreeMap<u64, u64>,
    pending_save: Vec<u64>,
    store: Option<WorkspaceStore>,
    workspace_dirty: bool,
    checkpoint_at: Instant,
    colors: (String, String, String, String),
    system_colors: Option<(String, String, String, String, String)>,
    selection_foreground: String,
    events: events::Events,
    revision: u64,
    files_revision: u64,
    git_revision: u64,
    render_cache: BTreeMap<u64, (String, u64, std::sync::Arc<Screen>)>,
    pub screen_builds: u64,
    typing: bool,
    git_branch: String,
    git_repository: bool,
    git_jobs: usize,
    git_error: String,
    /// Launched with files (or standard input) rather than a directory.
    pub file_mode: bool,
    /// The path whose recovery checkpoint this session uses.
    recovery_key: PathBuf,
    /// Quit once pending "save all" writes succeed.
    quit_after_save: bool,
    /// A save waiting for confirmation: (document, destination, overwrite).
    pending_retry: Option<(u64, Option<PathBuf>, bool)>,
    /// A privileged save the frontend must perform (terminal `sudo`).
    elevation: Option<Elevation>,
    /// How this frontend performs privileged saves.
    pub elevation_mode: ElevationMode,
    /// Consecutive line cuts append to the cut buffer, as in nano:
    /// (view, document generation, cursor) after the last cut.
    last_cut: Option<(u64, u64, usize)>,
    misspellings: Vec<String>,
    /// The terminal frontend should suspend itself (SIGTSTP).
    pub suspend_requested: bool,
    checkpoint_key: u64,
    /// The terminal interface hosts this session (enables suspend, mouse toggle).
    pub terminal_frontend: bool,
    disk_check_at: Instant,
    disk_check_pending: bool,
    disk_conflicts: std::collections::VecDeque<(u64, Document)>,
    recent: Vec<String>,
    overview_cache: BTreeMap<u64, desktop::Overview>,
    /// Requests only a frontend can fulfil (new window, print), drained by it.
    frontend_requests: Vec<String>,
    inbox: Option<instance::Inbox>,
    /// Cursor positions (line, column) for files whose open is in flight.
    pending_positions: BTreeMap<PathBuf, (usize, usize)>,
}
/// A privileged write the terminal frontend performs with `sudo`, after
/// handing it the terminal for a password prompt.
pub struct Elevation {
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
    pub fn new(path: &Path) -> Result<Self> {
        Self::new_with_startup(path, None)
    }
    /// Open one file (created on first save if missing) or directory.
    pub fn new_with_startup(path: &Path, mode: Option<StartupMode>) -> Result<Self> {
        let mut launch = cli::Launch {
            recover: true,
            startup: mode,
            ..Default::default()
        };
        if path.is_dir() {
            launch.directory = Some(path.to_path_buf());
        } else {
            launch.files.push(cli::LaunchFile {
                path: path.to_path_buf(),
                ..Default::default()
            });
        }
        Self::launch(&launch, None)
    }
    /// Build the initial session from parsed command-line arguments.
    /// `stdin` holds the bytes read for a `-` argument.
    pub fn launch(launch: &cli::Launch, stdin: Option<Vec<u8>>) -> Result<Self> {
        let directory = launch
            .directory
            .as_ref()
            .map(|d| {
                d.canonicalize()
                    .with_context(|| format!("Cannot open {}", d.display()))
            })
            .transpose()?;
        let mut opened: Vec<(Document, Option<usize>, Option<usize>)> = vec![];
        if let (Some(spec), Some(bytes)) = (&launch.stdin, stdin) {
            let mut document = Document::from_bytes(&bytes, "standard input")?;
            document.label = None;
            opened.push((document, spec.line, spec.column));
        }
        for file in &launch.files {
            let document = if file.path.exists() {
                Document::open(&file.path)
                    .with_context(|| format!("Cannot open {}", file.path.display()))?
            } else {
                Document::new_file(&file.path)?
            };
            opened.push((document, file.line, file.column));
        }
        let file_mode = directory.is_none() && (!launch.files.is_empty() || launch.stdin.is_some());
        let root = match &directory {
            Some(directory) => directory.clone(),
            None => opened
                .iter()
                .find_map(|(d, _, _)| d.path.as_ref()?.parent().map(Path::to_path_buf))
                .filter(|p| p.is_dir())
                .map_or_else(std::env::current_dir, Ok)?,
        };
        let recovery_key = match (&directory, opened.first()) {
            (None, Some((d, _, _))) => d.path.clone().unwrap_or_else(|| root.clone()),
            _ => root.clone(),
        };
        let mut notices = vec![];
        let mut documents = BTreeMap::new();
        let mut views = BTreeMap::new();
        let mut positions = vec![];
        let mut ids = 20;
        if opened.is_empty() {
            opened.push((Document::scratch(), None, None));
        }
        let mut tabs = vec![];
        for (index, (mut document, line, column)) in opened.into_iter().enumerate() {
            let (doc, view) = if index == 0 {
                (10, 11)
            } else {
                ids += 2;
                (ids - 1, ids)
            };
            if launch.read_only {
                document.read_only = true;
            }
            if let Some(notice) = document.notice.take() {
                notices.push(notice);
            }
            documents.insert(doc, document);
            views.insert(
                view,
                EditorView {
                    document: doc,
                    ..Default::default()
                },
            );
            tabs.push(View::Editor(view));
            positions.push((view, line, column));
        }
        let mut layout = Node::default_layout(11, 12);
        if let Some((pane_tabs, _)) = layout.pane_mut(2) {
            *pane_tabs = tabs;
        }
        let events = events::Events::default();
        let mut app = Self {
            browser: root.clone(),
            root: root.clone(),
            documents,
            views,
            terminals: BTreeMap::new(),
            deferred_terminals: BTreeMap::from([(12, root.clone())]),
            editor_only: false,
            layout,
            focus: 2,
            status: "F1 commands · F6 focus · Ctrl-S save".into(),
            quit: false,
            clipboard: String::new(),
            files: vec![],
            git: vec![],
            selected: 0,
            git_selected: 0,
            services: Services::new(events.clone()),
            events,
            revision: 1,
            files_revision: 1,
            git_revision: 1,
            render_cache: BTreeMap::new(),
            screen_builds: 0,
            typing: false,
            git_branch: String::new(),
            git_repository: false,
            git_jobs: 0,
            git_error: String::new(),
            ids,
            presets: BTreeMap::new(),
            preferences: Preferences::default(),
            prompt: None,
            search: Search::default(),
            tokens: BTreeMap::new(),
            highlight_pending: BTreeMap::new(),
            pending_open: BTreeMap::new(),
            pending_save: vec![],
            store: None,
            workspace_dirty: false,
            pending_close: None,
            checkpoint_at: Instant::now(),
            selection_foreground: "#ffffff".into(),
            colors: (
                "#d8dee9".into(),
                "#20242c".into(),
                "#425b78".into(),
                "#88c0d0".into(),
            ),
            system_colors: None,
            file_mode,
            recovery_key,
            quit_after_save: false,
            pending_retry: None,
            elevation: None,
            elevation_mode: ElevationMode::None,
            last_cut: None,
            misspellings: vec![],
            suspend_requested: false,
            checkpoint_key: 0,
            terminal_frontend: true,
            disk_check_at: Instant::now(),
            disk_check_pending: false,
            disk_conflicts: Default::default(),
            recent: desktop::load_recent(),
            overview_cache: BTreeMap::new(),
            frontend_requests: vec![],
            inbox: None,
            pending_positions: BTreeMap::new(),
        };
        app.read_settings();
        app.load_preferences();
        app.editor_only = launch.startup.unwrap_or(if file_mode {
            app.preferences.file_startup
        } else {
            app.preferences.directory_startup
        }) == StartupMode::EditorOnly;
        for (view, line, column) in positions {
            if line.is_some() || column.is_some() {
                let doc = app.views[&view].document;
                let cursor = app.documents[&doc].at_line_col(
                    line.unwrap_or(1).saturating_sub(1),
                    column.unwrap_or(1).saturating_sub(1),
                    app.preferences.indent_width,
                );
                app.views.get_mut(&view).unwrap().cursor = cursor;
            }
        }
        if !app.editor_only {
            app.ensure_terminals()?;
        }
        if !notices.is_empty() {
            app.status = notices.join(" · ");
        }
        app.refresh();
        Ok(app)
    }
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
        self.watch_files();
        self.schedule_highlight();
        if self.workspace_dirty && self.checkpoint_at.elapsed() >= Duration::from_secs(1) {
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
        if self.pending_save.contains(&doc) {
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
            self.tokens.remove(&doc);
            self.highlight_pending.remove(&doc);
        }
        if self.editor_only && !matches!(self.focused(), Some(View::Editor(_))) {
            self.expand_workspace()?;
        }
        self.status = format!("Closed {title}");
        Ok(())
    }
    fn add_tab(&mut self, view: View) {
        if let Some((tabs, active)) = self.layout.pane_mut(self.focus) {
            // Files and Git are singleton views within a pane. Reopening them
            // should reveal the existing tab instead of crowding the tab bar.
            if matches!(view, View::Files | View::Git) {
                if let Some(index) = tabs.iter().position(|tab| *tab == view) {
                    *active = index;
                    return;
                }
            }
            tabs.push(view);
            *active = tabs.len() - 1;
        }
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
        self.typing =
            matches!(&cmd, Command::Key { key } if !key.ctrl && !key.alt && !key.text.is_empty());
        if !self.typing {
            for doc in self.documents.values_mut() {
                doc.break_typing();
            }
        }
        self.revision += 1;
        if !matches!(cmd, Command::Theme { .. }) {
            self.workspace_dirty = true;
        }
        if let Err(e) = self.execute(cmd) {
            self.status = format!("Error: {e:#}");
        }
        self.typing = false;
        self.schedule_highlight();
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
                    let cursor = document.text[..view.cursor].encode_utf16().count();
                    let start = cursor
                        .checked_add_signed(replace_start as isize)
                        .context("Invalid input-method range")?;
                    let end = start
                        .checked_add(replace_length)
                        .context("Invalid input-method range")?;
                    let byte = |units: usize| -> Option<usize> {
                        let mut n = 0;
                        for (i, c) in document.text.char_indices() {
                            if n == units {
                                return Some(i);
                            }
                            n += c.len_utf16();
                        }
                        (n == units).then_some(document.text.len())
                    };
                    let (start, end) = (
                        byte(start).context("Invalid UTF-16 start")?,
                        byte(end).context("Invalid UTF-16 end")?,
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
            Command::Prompt { kind } => {
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
                                self.documents[&v.document].text[a.min(v.cursor)..a.max(v.cursor)]
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
            }
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
            Command::SubmitPrompt { all } => {
                let p = self.prompt.clone().context("No active prompt")?;
                match p.kind.as_str() {
                    // Graphical frontends answer confirmations through
                    // dialogs: `all` selects the alternative button.
                    "save-read-only" | "save-elevated" | "reload-changed" => {
                        self.prompt = None;
                        self.confirm_pending(&p.kind)?;
                    }
                    "file-changed" => self.resolve_disk_conflict(!all),
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
                        }
                    }
                    "open" | "open-folder" | "save-as" | "commit" | "layout-save"
                    | "layout-load" | "move-pane" | "settings" => {
                        if p.input.trim().is_empty() {
                            bail!("Enter {}", p.kind);
                        }
                        self.prompt = None;
                        self.command_line(&format!(
                            "{} {}",
                            if p.kind == "settings" { "set" } else { &p.kind },
                            p.input
                        ));
                        if self.status.starts_with("Error:") || self.status.starts_with("Unknown") {
                            self.prompt = Some(p);
                        }
                    }
                    "insert-file" | "set-encoding" | "reopen-encoding" | "set-line-ending"
                    | "open-recent" => {
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
            }
            Command::DismissPrompt => {
                let kind = self.prompt.take().map(|p| p.kind).unwrap_or_default();
                self.pending_close = None;
                match kind.as_str() {
                    "save-read-only" | "save-elevated" | "reload-changed" => {
                        self.pending_retry = None;
                        self.quit_after_save = false;
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
                if self.preferences.theme == "auto" {
                    self.colors = (foreground, background, selection, accent);
                    self.selection_foreground = selection_foreground;
                }
            }
            Command::ReloadSettings => self.load_preferences(),
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
            } => {
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
            }
            Command::Key { key } => self.key(key)?,
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
                Some(View::Editor(id)) => self.edit(id, &text_format::normalize_input(&text))?,
                _ => {}
            },
            Command::Click {
                pane,
                row,
                col,
                shift,
            } => {
                self.focus = pane;
                match self.focused() {
                    Some(View::Editor(id)) => {
                        let cursor = self.click_offset(id, row, col);
                        let v = self.views.get_mut(&id).unwrap();
                        v.manual_scroll = false;
                        v.goal = None;
                        if shift || v.mark {
                            v.anchor.get_or_insert(v.cursor);
                        } else {
                            v.anchor = None;
                        }
                        v.cursor = cursor;
                    }
                    Some(View::Files) => {
                        self.selected = row.min(self.files.len().saturating_sub(1))
                    }
                    Some(View::Git) => {
                        self.git_selected = row.min(self.git.len().saturating_sub(1))
                    }
                    Some(View::Terminal(id)) => {
                        self.terminals.get_mut(&id).unwrap().select(row, col, shift)
                    }
                    _ => {}
                }
            }
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
                self.pending_open.insert(token, self.focus);
                self.services
                    .io
                    .send(IoJob::Open(token, path))
                    .map_err(|e| anyhow::anyhow!(e.to_string()))?;
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
                let change = if matches!(cmd, Command::Undo) {
                    d.undo_change()
                } else {
                    d.redo_change()
                };
                let cursor = if matches!(cmd, Command::Undo) {
                    d.undo(view.cursor)
                } else {
                    d.redo(view.cursor)
                };
                if let Some(cursor) = cursor {
                    if let Some((start, old, new)) = change {
                        self.rebase_views(view.document, start, start + old, new);
                    }
                    let v = self.views.get_mut(&id).unwrap();
                    v.cursor = cursor;
                    v.anchor = None;
                }
            }
            Command::SelectAll => {
                if let Some(id) = self.active_editor() {
                    let v = self.views.get_mut(&id).unwrap();
                    v.anchor = Some(0);
                    v.cursor = self.documents[&v.document].text.len();
                }
            }
            Command::Copy | Command::Cut => {
                if let Some(View::Terminal(id)) = self.focused() {
                    self.clipboard = self.terminals[&id].selection_text();
                }
                if let Some(id) = self.active_editor() {
                    let v = &self.views[&id];
                    if let Some(anchor) = v.anchor {
                        self.clipboard = self.documents[&v.document].text
                            [v.cursor.min(anchor)..v.cursor.max(anchor)]
                            .to_string();
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
            Command::NextTab => {
                let next = self.layout.pane_mut(self.focus).map(|(tabs, active)| {
                    let index = (*active + 1) % tabs.len();
                    (index, !matches!(tabs[index], View::Editor(_)))
                });
                if let Some((index, tool)) = next {
                    if self.editor_only && tool {
                        self.expand_workspace()?;
                    }
                    *self.layout.pane_mut(self.focus).unwrap().1 = index;
                }
            }
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
                if self.layout.remove(self.focus) {
                    self.focus = self.layout.panes()[0];
                } else {
                    bail!("Cannot close the final pane");
                }
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
                self.write_settings()?;
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
            Command::GitStage { path } => {
                let mut args = vec!["add".into(), "--all".into(), "--".into()];
                if let Some(entry) = self.git.iter().find(|e| e.path == path) {
                    args.extend(entry.staging_paths());
                } else {
                    args.push(path);
                }
                self.git_action(args)?;
            }
            Command::GitUnstage { path } => {
                let mut args = vec!["reset".into(), "HEAD".into(), "--".into(), path.clone()];
                if let Some(old) = self
                    .git
                    .iter()
                    .find(|e| e.path == path)
                    .and_then(|e| e.original_path.clone())
                {
                    args.push(old);
                }
                self.git_action(args)?;
            }
            Command::GitStageAll => {
                self.git_action(vec!["add".into(), "--all".into(), "--".into(), ".".into()])?;
            }
            Command::GitUnstageAll => {
                self.git_action(vec!["reset".into(), "HEAD".into(), "--".into(), ".".into()])?;
            }
            Command::GitStageGroup { group } => {
                let paths: std::collections::BTreeSet<String> = self
                    .git
                    .iter()
                    .filter(|entry| entry.group == group && !entry.staged)
                    .flat_map(GitEntry::staging_paths)
                    .collect();
                if paths.is_empty() {
                    bail!("No unstaged changes in {group}");
                }
                let mut args = vec!["add".into(), "--all".into(), "--".into()];
                args.extend(paths);
                self.git_action(args)?;
            }
            Command::GitDiff { path, staged } => {
                let untracked = self.git.iter().any(|e| e.path == path && e.untracked);
                self.services
                    .tx
                    .send(Job::GitDiff(self.root.clone(), path, staged, untracked))?;
                self.git_jobs += 1;
                self.git_error.clear();
                self.status = "Loading Git diff…".into();
            }
            Command::GitCommit { message } => {
                if message.trim().is_empty() {
                    bail!("Commit message is empty");
                }
                self.git_action(vec!["commit".into(), "-m".into(), message])?;
            }
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
                if !self.pending_save.is_empty() {
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
        let path = path.canonicalize()?;
        if !path.is_dir() {
            bail!("Not a directory");
        }
        self.browser = path.clone();
        self.selected = 0;
        self.services.tx.send(Job::Browse(path))?;
        Ok(())
    }
    pub fn refresh(&mut self) {
        let _ = self.services.tx.send(Job::Browse(self.browser.clone()));
        self.refresh_git();
    }
    fn refresh_git(&mut self) {
        if self
            .services
            .tx
            .send(Job::GitStatus(self.root.clone()))
            .is_ok()
        {
            self.git_jobs += 1;
        }
    }
    fn git_action(&mut self, args: Vec<String>) -> Result<()> {
        self.services.tx.send(Job::Git(self.root.clone(), args))?;
        self.git_error.clear();
        self.git_jobs += 1;
        self.refresh_git();
        self.status = "Running Git…".into();
        Ok(())
    }
    pub fn poll(&mut self) {
        while let Ok(reply) = self.services.rx.try_recv() {
            self.revision += 1;
            match reply {
                Reply::OpenedDirectory(token, path) => {
                    if let Some(pane) = self.pending_open.remove(&token) {
                        if self.layout.view(pane).is_some() {
                            self.focus = pane;
                        }
                        if let Err(e) = self.browse(path) {
                            self.status = format!("Browse failed: {e:#}");
                        }
                    }
                }
                Reply::Opened(token, result) => {
                    if let Some(pane) = self.pending_open.remove(&token) {
                        match result {
                            Ok(d) => self.finish_open(pane, d),
                            Err(e) => self.status = format!("Open failed: {e}"),
                        }
                    }
                }
                Reply::Saved(id, result) => {
                    self.pending_save.retain(|d| *d != id);
                    match result {
                        Ok(d) => {
                            if let Some(path) = d.path.clone() {
                                self.remember_recent(&path);
                            }
                            if let Some(doc) = self.documents.get_mut(&id) {
                                doc.accept_save(d);
                                self.status = if doc.dirty() {
                                    "Saved snapshot; newer edits remain unsaved"
                                } else {
                                    "Saved"
                                }
                                .into();
                                self.workspace_dirty = true;
                                self.refresh();
                            }
                            if self.quit_after_save {
                                if let Err(e) = self.save_all(true) {
                                    self.quit_after_save = false;
                                    self.status = format!("Error: {e:#}");
                                }
                            }
                        }
                        Err(failure) => self.save_failed(id, failure),
                    }
                }
                Reply::Reloaded(id, result) => match result {
                    Ok(d) => self.finish_reload(id, d),
                    Err(e) => self.status = format!("Reload failed: {e}"),
                },
                Reply::Highlighted(id, generation, light, tokens) => {
                    self.highlight_pending.remove(&id);
                    if self
                        .documents
                        .get(&id)
                        .is_some_and(|d| d.generation == generation)
                        && light == self.light_theme()
                    {
                        self.tokens.insert(id, (generation, light, tokens));
                    }
                }
                Reply::Files(path, Ok(entries)) if path == self.browser => {
                    if self.files != entries {
                        self.files_revision += 1;
                    }
                    self.files = entries;
                    self.selected = self.selected.min(self.files.len().saturating_sub(1));
                }
                Reply::Files(_, Err(e)) => self.status = e,
                Reply::Git(Ok(state)) => {
                    self.git_jobs = self.git_jobs.saturating_sub(1);
                    self.git_revision += 1;
                    self.git_branch = state.branch;
                    self.git_repository = state.repository;
                    let selection = self
                        .git
                        .get(self.git_selected)
                        .map(|e| (e.path.clone(), e.staged));
                    self.git = state.entries;
                    if let Some((path, staged)) = selection {
                        if let Some(index) = self
                            .git
                            .iter()
                            .position(|e| e.path == path && e.staged == staged)
                            .or_else(|| self.git.iter().position(|e| e.path == path))
                        {
                            self.git_selected = index;
                        }
                    }
                    self.git_selected = self.git_selected.min(self.git.len().saturating_sub(1));
                }
                Reply::Git(Err(e)) => {
                    self.git_jobs = self.git_jobs.saturating_sub(1);
                    self.git_error = e.clone();
                    self.status = format!("Git: {e}");
                }
                Reply::GitOperation(result) => {
                    self.git_jobs = self.git_jobs.saturating_sub(1);
                    match result {
                        Ok(message) => {
                            self.status = if message.is_empty() {
                                "Git operation completed".into()
                            } else {
                                message.lines().next().unwrap_or_default().into()
                            }
                        }
                        Err(e) => {
                            self.git_error = e.clone();
                            self.status = format!("Git: {e}");
                        }
                    }
                }
                Reply::Error(e) => self.status = e,
                Reply::Spelling(result) => self.spelling_result(result),
                Reply::Disk(changes) => self.disk_changes(changes),
                Reply::Output(_, _) if self.quit => {
                    self.git_jobs = self.git_jobs.saturating_sub(1);
                }
                Reply::Output(title, text) => {
                    self.git_jobs = self.git_jobs.saturating_sub(1);
                    let doc = self.id();
                    let d = Document::inspection(format!("{title} · read-only"), text);
                    self.documents.insert(doc, d);
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
                    self.status = "Git diff · read-only".into();
                }
                _ => {}
            }
        }
    }
    fn new_terminal(&mut self) -> Result<u64> {
        let id = self.id();
        let mut terminal = TerminalSession::spawn(&self.root, None, 24, 80)?;
        terminal.set_events(self.events.clone());
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
        self.focus = self
            .layout
            .panes()
            .into_iter()
            .find(|p| matches!(self.layout.view(*p), Some(View::Editor(_))))
            .unwrap();
        Ok(())
    }
    fn settings_path() -> PathBuf {
        paths::config_dir().join("layouts.toml")
    }
    fn read_settings(&mut self) {
        if let Ok(s) = fs::read_to_string(Self::settings_path()) {
            if let Ok(settings) = toml::from_str::<Settings>(&s) {
                self.presets = settings
                    .presets
                    .into_iter()
                    .filter(|(_, n)| n.validate())
                    .collect();
            }
        }
    }
    fn write_settings(&self) -> Result<()> {
        let path = Self::settings_path();
        fs::create_dir_all(path.parent().unwrap())?;
        use std::io::Write;
        let mut temp = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
        temp.write_all(
            toml::to_string_pretty(&Settings {
                presets: self.presets.clone(),
            })?
            .as_bytes(),
        )?;
        temp.as_file().sync_all()?;
        temp.persist(path).map_err(|e| e.error)?;
        Ok(())
    }
    fn restore_layout(&mut self, mut node: Node) -> Result<()> {
        if !node.validate() {
            bail!("Invalid saved layout");
        }
        // Saved layouts persist shape and view kinds; processes/documents are rebound for this launch.
        let doc = self.views.values().next().unwrap().document;
        fn rebind(app: &mut App, n: &mut Node, doc: u64) -> Result<()> {
            match n {
                Node::Pane { id, tabs, .. } => {
                    *id = app.id();
                    for v in tabs {
                        match v {
                            View::Editor(id) => {
                                *id = app.id();
                                app.views.insert(
                                    *id,
                                    EditorView {
                                        document: doc,
                                        ..Default::default()
                                    },
                                );
                            }
                            View::Terminal(id) => {
                                *id = app.new_terminal()?;
                            }
                            _ => {}
                        }
                    }
                }
                Node::Split {
                    id, first, second, ..
                } => {
                    *id = app.id();
                    rebind(app, first, doc)?;
                    rebind(app, second, doc)?;
                }
            }
            Ok(())
        }
        rebind(self, &mut node, doc)?;
        self.editor_only = false;
        self.layout = node;
        self.focus = self.layout.panes()[0];
        Ok(())
    }
    fn clamp_views(&mut self, doc: u64) {
        let text = &self.documents[&doc].text;
        for v in self.views.values_mut().filter(|v| v.document == doc) {
            v.cursor = v.cursor.min(text.len());
            while !text.is_char_boundary(v.cursor) {
                v.cursor -= 1;
            }
            v.anchor = v.anchor.map(|a| {
                let mut a = a.min(text.len());
                while !text.is_char_boundary(a) {
                    a -= 1;
                }
                a
            });
            v.top = v.top.min(text.bytes().filter(|b| *b == b'\n').count());
        }
    }
    fn edit(&mut self, id: u64, value: &str) -> Result<()> {
        let v = self.views[&id].clone();
        let anchor = v.anchor.unwrap_or(v.cursor);
        let start = v.cursor.min(anchor);
        let end = v.cursor.max(anchor);
        self.edit_range(id, start, end, value, v.cursor)
    }
    fn rebase_views(&mut self, doc: u64, start: usize, end: usize, length: usize) {
        let position = |p: usize| {
            if p <= start {
                p
            } else if p >= end {
                p - end + start + length
            } else {
                start + length
            }
        };
        for other in self.views.values_mut().filter(|o| o.document == doc) {
            other.cursor = position(other.cursor);
            other.anchor = other.anchor.map(position);
        }
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
        self.rebase_views(doc, start, end, value.len());
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
        self.preferences
            .global_keys
            .get(&chord)
            .or_else(|| match self.focused_kind() {
                "editor" => self.preferences.editor_keys.get(&chord),
                "terminal" => self.preferences.terminal_keys.get(&chord),
                _ => None,
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
            "close-tab" => match k.key.as_str() {
                "Escape" | "Enter" => self.execute(Command::DismissPrompt)?,
                _ if letter == "d" => self.execute(Command::SubmitPrompt { all: true })?,
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
                    self.pending_retry = None;
                    self.quit_after_save = false;
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
                    self.git.len()
                } else {
                    self.files.len()
                };
                let selected = if git {
                    &mut self.git_selected
                } else {
                    &mut self.selected
                };
                match k.key.as_str() {
                    "Up" => *selected = selected.saturating_sub(1),
                    "Down" => *selected = (*selected + 1).min(length.saturating_sub(1)),
                    "Home" => *selected = 0,
                    "End" => *selected = length.saturating_sub(1),
                    "Enter" => {
                        if git {
                            if self.git.get(*selected).is_some() {
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
                if let Some(name) = name {
                    return self.move_cursor(id, name, k.shift);
                }
                let v = self.views[&id].clone();
                let text = &self.documents[&v.document].text;
                let (_, col) =
                    self.documents[&v.document].line_col(v.cursor, self.preferences.indent_width);
                match k.key.as_str() {
                    "Backspace" if k.ctrl && v.anchor.is_none() => {
                        let start = actions::word_boundary(text, v.cursor, false);
                        self.edit_range(id, start, v.cursor, "", v.cursor)?;
                    }
                    "Delete" if k.ctrl && v.anchor.is_none() => {
                        let end = actions::word_boundary(text, v.cursor, true);
                        self.edit_range(id, v.cursor, end, "", v.cursor)?;
                    }
                    "Backspace" => {
                        if v.anchor.is_none() && v.cursor > 0 {
                            self.views.get_mut(&id).unwrap().anchor =
                                Some(previous(text, v.cursor));
                        }
                        if self.views[&id].anchor.is_some() {
                            self.edit(id, "")?;
                        }
                    }
                    "Delete" => {
                        if v.anchor.is_none() && v.cursor < text.len() {
                            self.views.get_mut(&id).unwrap().anchor = Some(next(text, v.cursor));
                        }
                        if self.views[&id].anchor.is_some() {
                            self.edit(id, "")?;
                        }
                    }
                    "Enter" => {
                        let leading = text[line_start(text, v.cursor)..v.cursor]
                            .chars()
                            .take_while(|c| *c == ' ' || *c == '\t')
                            .collect::<String>();
                        let value = format!(
                            "\n{}",
                            if self.preferences.auto_indent {
                                leading.as_str()
                            } else {
                                ""
                            }
                        );
                        self.edit(id, &value)?;
                    }
                    "Tab" => {
                        if k.shift
                            || v.anchor.is_some_and(|a| {
                                text[a.min(v.cursor)..a.max(v.cursor)].contains('\n')
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
                    }
                    _ if !k.ctrl && !k.alt && !k.text.is_empty() => {
                        let typed = text_format::normalize_input(&k.text).into_owned();
                        self.edit(id, &typed)?;
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
    pub fn snapshot(
        &mut self,
        width: u16,
        height: u16,
        gap: u16,
        cell_width: u16,
        cell_height: u16,
        header: u16,
    ) -> Snapshot {
        self.snapshot_with_minimum(
            Rect {
                x: 0,
                y: 0,
                width,
                height,
            },
            gap,
            (cell_width, cell_height),
            header,
            (0, 0),
        )
    }
    pub fn snapshot_with_minimum(
        &mut self,
        area: Rect,
        gap: u16,
        cell: (u16, u16),
        header: u16,
        minimum: (u16, u16),
    ) -> Snapshot {
        self.snapshot_with_revisions(area, gap, cell, header, minimum, None)
    }
    pub fn snapshot_with_revisions(
        &mut self,
        area: Rect,
        gap: u16,
        cell: (u16, u16),
        header: u16,
        minimum: (u16, u16),
        known: Option<(u64, u64)>,
    ) -> Snapshot {
        self.snapshot_presentation(area, gap, cell, header, minimum, known, false)
    }
    pub fn gui_snapshot(
        &mut self,
        area: Rect,
        gap: u16,
        cell: (u16, u16),
        header: u16,
        minimum: (u16, u16),
        known: Option<(u64, u64)>,
    ) -> Snapshot {
        self.snapshot_presentation(area, gap, cell, header, minimum, known, true)
    }
    #[allow(clippy::too_many_arguments)]
    fn snapshot_presentation(
        &mut self,
        area: Rect,
        gap: u16,
        cell: (u16, u16),
        header: u16,
        minimum: (u16, u16),
        known: Option<(u64, u64)>,
        graphical: bool,
    ) -> Snapshot {
        let Rect { width, height, .. } = area;
        let (cell_width, cell_height) = cell;
        self.process_events();
        self.render_cache
            .retain(|id, _| self.views.contains_key(id) || self.terminals.contains_key(id));
        let cw = cell_width.max(1);
        let ch = cell_height.max(1);
        let required = self.layout.minimum_size(gap, minimum);
        let (placements, handles) = if self.editor_only
            || width / cw < 70
            || height / ch < 10
            || width < required.0
            || height < required.1
        {
            (
                vec![layout::Placement {
                    id: self.focus,
                    rect: area,
                }],
                vec![],
            )
        } else {
            self.layout.arrange_constrained(area, gap, minimum)
        };
        let mut panes = vec![];
        for placement in placements {
            let id = placement.id;
            let r = placement.rect;
            let Some(view) = self.layout.view(id).cloned() else {
                continue;
            };
            let tabs = match self.layout.pane_mut(id) {
                Some((tabs, active)) => {
                    let active = *active;
                    tabs.clone()
                        .iter()
                        .enumerate()
                        .map(|(index, v)| Tab {
                            title: match v {
                                View::Files => "Files".into(),
                                View::Git => "Git".into(),
                                View::Editor(id) => self
                                    .views
                                    .get(id)
                                    .and_then(|v| self.documents.get(&v.document))
                                    .map(Document::title)
                                    .unwrap_or_else(|| "Editor".into()),
                                View::Terminal(id) => format!("Terminal {id}"),
                            },
                            active: index == active,
                            editor_id: match v {
                                View::Editor(id) => Some(*id),
                                _ => None,
                            },
                            close_id: match v {
                                View::Editor(id) | View::Terminal(id) => Some(*id),
                                _ => None,
                            },
                        })
                        .collect()
                }
                None => vec![],
            };
            let rows = r
                .height
                .saturating_sub(header)
                .checked_div(ch)
                .unwrap_or(1)
                .max(1);
            let cols = r
                .width
                .saturating_sub(2)
                .checked_div(cw)
                .unwrap_or(1)
                .max(1);
            let mut text = None;
            let (kind, screen, selected) = match view {
                View::Editor(view) => {
                    if graphical {
                        text = Some(self.editor_text(view, rows, cols));
                        ("editor", None, 0)
                    } else {
                        let screen = self.cached_editor_screen(view, rows, cols);
                        ("editor", Some(screen), 0)
                    }
                }
                View::Terminal(term) => {
                    let screen = self.terminals.get_mut(&term).map(|t| {
                        let _ = t.resize(rows, cols);
                        // The signature is checked below before the parser grid is copied.
                        t.revision()
                    });
                    let screen = screen.map(|revision| self.cached_terminal_screen(term, revision));
                    ("terminal", screen, 0)
                }
                View::Files => ("files", None, self.selected),
                View::Git => ("git", None, self.git_selected),
            };
            panes.push(PaneSnapshot {
                id,
                rect: r,
                kind: kind.into(),
                tabs,
                revision: self
                    .render_cache
                    .get(&match view {
                        View::Editor(v) | View::Terminal(v) => v,
                        _ => 0,
                    })
                    .map(|(_, revision, _)| *revision)
                    .unwrap_or(0),
                focused: id == self.focus,
                terminal_mouse_motion: match view {
                    View::Terminal(v) => self.terminals[&v].mouse_motion(),
                    _ => false,
                },
                read_only: match view {
                    View::Editor(v) => self.documents[&self.views[&v].document].read_only,
                    _ => false,
                },
                editor: match view {
                    View::Editor(v) => Some(self.editor_presentation(v)),
                    _ => None,
                },
                screen,
                selected,
                rows,
                cols,
                text,
            });
        }
        Snapshot {
            panes,
            handles,
            focus: self.focus,
            files: if known.is_some_and(|(files, _)| files == self.files_revision) {
                vec![]
            } else {
                self.files.clone()
            },
            git: if known.is_some_and(|(_, git)| git == self.git_revision) {
                vec![]
            } else {
                self.git.clone()
            },
            browser: self.browser.to_string_lossy().into_owned(),
            status: self.status.clone(),
            dirty: self.dirty(),
            quit: self.quit,
            clipboard: if graphical {
                String::new()
            } else {
                self.clipboard.clone()
            },
            layouts: self.presets.keys().cloned().collect(),
            prompt: self.prompt.clone(),
            location: self.location(),
            hints: self.hints(),
            foreground: self.colors.0.clone(),
            background: self.colors.1.clone(),
            accent: self.colors.3.clone(),
            selection: self.colors.2.clone(),
            commands: if graphical {
                vec![]
            } else {
                self.command_catalog("")
            },
            settings: self.preferences.clone(),
            editor_only: self.editor_only,
            title: self.window_title(),
            recent: self.recent.clone(),
            git_branch: self.git_branch.clone(),
            git_repository: self.git_repository,
            git_busy: self.git_jobs > 0,
            git_error: self.git_error.clone(),
            files_revision: self.files_revision,
            git_revision: self.git_revision,
        }
    }
    pub fn editor_input_context(&self, pane: u64) -> Option<EditorPresentation> {
        match self.layout.view(pane) {
            Some(View::Editor(id)) if self.views.contains_key(id) => {
                Some(self.editor_presentation(*id))
            }
            _ => None,
        }
    }
    fn editor_presentation(&self, id: u64) -> EditorPresentation {
        let v = &self.views[&id];
        let doc = &self.documents[&v.document];
        let mut start = v.cursor.saturating_sub(2048);
        while !doc.text.is_char_boundary(start) {
            start += 1;
        }
        let mut end = (v.cursor + 2048).min(doc.text.len());
        while !doc.text.is_char_boundary(end) {
            end -= 1;
        }
        let anchor = v.anchor.unwrap_or(v.cursor).clamp(start, end);
        let selected = v
            .anchor
            .map(|a| &doc.text[a.min(v.cursor)..a.max(v.cursor)])
            .unwrap_or("");
        EditorPresentation {
            surrounding: doc.text[start..end].into(),
            cursor: doc.text[start..v.cursor].encode_utf16().count(),
            anchor: doc.text[start..anchor].encode_utf16().count(),
            selection: selected.chars().take(2048).collect(),
            line_count: doc.line_count(),
            top: v.top,
            left: v.left,
            document: v.document,
            generation: doc.generation,
        }
    }
    fn editor_signature(&self, id: u64) -> String {
        let v = &self.views[&id];
        serde_json::to_string(&(
            v,
            self.documents[&v.document].generation,
            self.tokens
                .get(&v.document)
                .map(|(generation, light, _)| (*generation, *light)),
            &self.colors,
            &self.selection_foreground,
            &self.search,
            self.preferences.line_numbers,
            self.preferences.indent_width,
            self.preferences.soft_wrap,
            self.preferences.show_whitespace,
        ))
        .unwrap()
    }
    fn cached_editor_screen(&mut self, id: u64, rows: u16, cols: u16) -> std::sync::Arc<Screen> {
        let v = self.views.get_mut(&id).unwrap();
        v.rows = rows;
        v.cols = cols;
        let key = self.editor_signature(id);
        if let Some((prior, _, screen)) = self.render_cache.get(&id) {
            if *prior == key {
                return screen.clone();
            }
        }
        let screen = std::sync::Arc::new(self.editor_screen(id, rows, cols));
        self.screen_builds += 1;
        self.render_cache.insert(
            id,
            (
                self.editor_signature(id),
                self.screen_builds,
                screen.clone(),
            ),
        );
        screen
    }
    fn cached_terminal_screen(&mut self, id: u64, revision: u64) -> std::sync::Arc<Screen> {
        let key = format!("terminal:{revision}:{}:{}", self.colors.0, self.colors.1);
        if let Some((prior, _, screen)) = self.render_cache.get(&id) {
            if *prior == key {
                return screen.clone();
            }
        }
        let screen =
            std::sync::Arc::new(self.terminals[&id].screen_with(&self.colors.0, &self.colors.1));
        self.screen_builds += 1;
        self.render_cache
            .insert(id, (key, self.screen_builds, screen.clone()));
        screen
    }
    fn editor_screen(&mut self, id: u64, rows: u16, cols: u16) -> Screen {
        self.render_editor(id, rows, cols, false).0
    }
    fn render_editor(
        &mut self,
        id: u64,
        rows: u16,
        cols: u16,
        graphical: bool,
    ) -> (Screen, Vec<Vec<text_presentation::TextSpan>>) {
        let tab = self.preferences.indent_width;
        let wrapping = self.preferences.soft_wrap;
        {
            let v = self.views.get_mut(&id).unwrap();
            v.rows = rows;
            v.cols = cols;
        }
        let doc_id = self.views[&id].document;
        let gutter = self.gutter_width(doc_id, cols);
        let usable = (cols as usize).saturating_sub(gutter).max(1);
        self.ensure_visible(id);
        let (cursor_line, cursor_row, cursor_col) =
            self.visual_position(id, self.views[&id].cursor);
        if !wrapping {
            // Horizontal scrolling keeps the cursor column visible, unless the
            // user scrolled away deliberately (wheel or scrollbar).
            let v = self.views.get_mut(&id).unwrap();
            if v.manual_scroll {
            } else if cursor_col < v.left {
                v.left = cursor_col;
            } else if cursor_col >= v.left + usable {
                v.left = cursor_col + 1 - usable;
            }
        } else {
            self.views.get_mut(&id).unwrap().left = 0;
        }
        // Graphical views draw one extra row for pixel-smooth scrolling.
        let render_rows = rows as usize + usize::from(graphical);
        let visible = self.visible_rows(id, render_rows);
        let show_whitespace = self.preferences.show_whitespace;
        let whitespace_color = desktop::blend(&self.colors.0, &self.colors.1, 0.6);
        let v = &self.views[&id];
        let document = &self.documents[&doc_id];
        let text = &document.text;
        let left = if wrapping { 0 } else { v.left };
        let selection = v.anchor.map(|a| (a.min(v.cursor), a.max(v.cursor)));
        let mut blank = Cell::text(" ");
        blank.fg = self.colors.0.clone();
        blank.bg = self.colors.1.clone();
        let mut cells = if graphical {
            vec![vec![]; render_rows]
        } else {
            vec![vec![blank; cols as usize]; rows as usize]
        };
        let mut decorations = vec![vec![]; render_rows];
        let token_data = self
            .tokens
            .get(&doc_id)
            .filter(|(generation, _, _)| *generation == document.generation);
        let tokens = token_data.map(|(_, _, t)| t.as_slice()).unwrap_or(&[]);
        let matches = self.search.regex().ok();
        let match_margin = self.search.query.len() * 4 + 4;
        let mut cursor = None;
        for (y, (line, row_index, row)) in visible.iter().enumerate() {
            let (line_start_byte, line_end_byte) = document.line_range(*line);
            let line_text = &text[line_start_byte..line_end_byte];
            if graphical {
                cells[y].resize_with(gutter, || Cell::text(" "));
            }
            if gutter > 0 {
                let number = if row.start == 0 {
                    format!("{:>width$}  ", line + 1, width = gutter.saturating_sub(2))
                } else {
                    " ".repeat(gutter)
                };
                for (x, c) in number.chars().take(gutter).enumerate() {
                    cells[y][x] = Cell::text(c.to_string());
                    cells[y][x].fg = self.colors.0.clone();
                    cells[y][x].bg = self.colors.1.clone();
                }
            }
            // The first byte to draw: the row start, or the horizontal scroll column.
            let first = if wrapping {
                row.start
            } else {
                document::column_offset(line_text, left, tab)
            };
            let mut x = if wrapping {
                row.col
            } else {
                document::display_width_with_tabs(&line_text[..first], tab)
            };
            let origin = if wrapping { row.col } else { left };
            let segment_end = if wrapping { row.end } else { line_text.len() };
            // Search only near the visible part of very long lines.
            let window_end = {
                let mut end = (first + usable * 4 + match_margin).min(segment_end);
                while !line_text.is_char_boundary(end) {
                    end += 1;
                }
                end
            };
            let window_start = {
                let mut start = first.saturating_sub(match_margin);
                while !line_text.is_char_boundary(start) {
                    start -= 1;
                }
                start
            };
            let ranges = matches
                .as_ref()
                .map(|r| {
                    r.find_iter(&line_text[window_start..window_end])
                        .map(|m| (window_start + m.start(), window_start + m.end()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let mut match_index = 0;
            let mut token_index = tokens.partition_point(|t| t.end <= line_start_byte + first);
            for (offset_in_segment, g) in line_text[first..segment_end].grapheme_indices(true) {
                let i = first + offset_in_segment;
                if g == "\r" || g == "\r\n" {
                    continue;
                }
                let width = document::grapheme_width(g, x, tab);
                for offset in 0..width {
                    let column = x + offset;
                    if column >= origin && column - origin + gutter < cols as usize {
                        let mut c = Cell::text(if g == "\t" || offset > 0 {
                            " ".to_string()
                        } else {
                            g.chars()
                                .map(|c| if c.is_control() { '\u{fffd}' } else { c })
                                .collect()
                        });
                        c.fg = self.colors.0.clone();
                        c.bg = self.colors.1.clone();
                        let absolute = line_start_byte + i;
                        while token_index < tokens.len() && tokens[token_index].end <= absolute {
                            token_index += 1;
                        }
                        if let Some(token) = tokens.get(token_index).filter(|t| t.start <= absolute)
                        {
                            c.fg = token.fg.clone();
                            c.bold = token.bold;
                            c.italic = token.italic;
                        }
                        while match_index < ranges.len() && ranges[match_index].1 <= i {
                            match_index += 1;
                        }
                        let matched = ranges
                            .get(match_index)
                            .is_some_and(|(a, b)| i >= *a && i < *b);
                        if show_whitespace && (g == " " || g == "\t") {
                            c.text = match (g, offset) {
                                (" ", _) => "·",
                                (_, 0) => "→",
                                _ => " ",
                            }
                            .into();
                            c.fg = whitespace_color.clone();
                        }
                        c.wide = width == 2 && g != "\t" && offset == 0;
                        c.continuation = width == 2 && g != "\t" && offset == 1;
                        let selected = selection
                            .map(|(a, b)| absolute >= a && absolute < b)
                            .unwrap_or(false);
                        let position = column - origin + gutter;
                        if matched || selected {
                            if graphical {
                                let mut decoration = c.clone();
                                decoration.bg = self.colors.2.clone();
                                decoration.fg = self.selection_foreground.clone();
                                decoration.underline = matched;
                                let row: &mut Vec<text_presentation::TextSpan> =
                                    &mut decorations[y];
                                if let Some(last) = row.last_mut().filter(|s| {
                                    s.start + s.length == position && s.underline == matched
                                }) {
                                    last.length += 1;
                                } else {
                                    row.push(text_presentation::TextSpan::cell(
                                        position,
                                        1,
                                        &decoration,
                                    ));
                                }
                            } else {
                                c.bg = self.colors.2.clone();
                                c.fg = self.selection_foreground.clone();
                                c.underline = matched;
                            }
                        }
                        if graphical && cells[y].len() <= position {
                            cells[y].resize_with(position + 1, || Cell::text(" "));
                        }
                        cells[y][position] = c;
                    }
                }
                x += width;
                if x >= origin + usable {
                    break;
                }
            }
            if *line == cursor_line && *row_index == cursor_row {
                let column = if wrapping {
                    cursor_col
                } else {
                    cursor_col.saturating_sub(left)
                };
                if wrapping || cursor_col >= left {
                    cursor = Some((
                        y as u16,
                        (column + gutter).min((cols as usize).saturating_sub(1)) as u16,
                    ));
                }
            }
        }
        if graphical {
            cells.truncate(visible.len());
            decorations.truncate(visible.len());
        }
        (
            Screen {
                cells,
                cursor,
                rows,
                cols,
            },
            decorations,
        )
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
            self.git.get(self.git_selected).map(|e| e.path.clone())
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
                staged: self.git.get(self.git_selected).is_some_and(|e| e.staged),
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
        }
        let path = path.unwrap();
        if let Some((line, column)) = self.pending_positions.remove(&path) {
            if let Some(View::Editor(view)) = self.layout.view(self.focus).cloned() {
                let doc = self.views[&view].document;
                let cursor = self.documents[&doc].at_line_col(
                    line.saturating_sub(1),
                    column.saturating_sub(1),
                    self.preferences.indent_width,
                );
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
        self.workspace_dirty = true;
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
