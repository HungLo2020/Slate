pub mod document;
pub mod layout;
mod services;
pub mod terminal;

use anyhow::{bail, Context, Result};
use document::{at_line_col, line_col, line_end, line_start, next, previous, Document};
use layout::{Axis, Handle, Node, Rect, View};
use serde::{Deserialize, Serialize};
use services::{Entry, GitEntry, Job, Reply, Services};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
use terminal::{Cell, Screen, TerminalSession};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Default)]
pub struct EditorView {
    pub document: u64,
    pub cursor: usize,
    pub anchor: Option<usize>,
    pub top: usize,
    pub left: usize,
    pub rows: u16,
    pub cols: u16,
    pub manual_scroll: bool,
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
    },
    New,
    CloseDocument {
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
    GitDiff {
        path: String,
    },
    GitCommit {
        message: String,
    },
    Quit {
        #[serde(default)]
        force: bool,
    },
}
#[derive(Serialize)]
pub struct Tab {
    pub title: String,
    pub active: bool,
}
#[derive(Serialize)]
pub struct PaneSnapshot {
    pub id: u64,
    pub rect: Rect,
    pub kind: String,
    pub tabs: Vec<Tab>,
    pub screen: Option<Screen>,
    pub selected: usize,
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
}
impl App {
    pub fn new(path: &Path) -> Result<Self> {
        let path = if path.exists() {
            path.canonicalize()?
        } else {
            bail!("Path does not exist: {}", path.display())
        };
        let file = path.is_file();
        let root = if file {
            path.parent().unwrap().to_path_buf()
        } else {
            path.clone()
        };
        let mut documents = BTreeMap::new();
        documents.insert(
            10,
            if file {
                Document::open(&path)?
            } else {
                Document::scratch()
            },
        );
        let mut views = BTreeMap::new();
        views.insert(
            11,
            EditorView {
                document: 10,
                ..Default::default()
            },
        );
        let mut terminals = BTreeMap::new();
        terminals.insert(12, TerminalSession::spawn(&root, None, 24, 80)?);
        let mut app = Self {
            browser: root.clone(),
            root,
            documents,
            views,
            terminals,
            layout: Node::default_layout(11, 12),
            focus: 2,
            status: "F1 commands · F6 focus · Ctrl-S save".into(),
            quit: false,
            clipboard: String::new(),
            files: vec![],
            git: vec![],
            selected: 0,
            git_selected: 0,
            services: Services::new(),
            ids: 20,
            presets: BTreeMap::new(),
        };
        app.read_settings();
        app.refresh();
        Ok(app)
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
    fn add_tab(&mut self, view: View) {
        if let Some((tabs, active)) = self.layout.pane_mut(self.focus) {
            tabs.push(view);
            *active = tabs.len() - 1;
        }
    }
    pub fn dispatch(&mut self, cmd: Command) {
        if let Err(e) = self.execute(cmd) {
            self.status = format!("Error: {e:#}");
        }
    }
    fn execute(&mut self, cmd: Command) -> Result<()> {
        match cmd {
            Command::Key { key } => self.key(key)?,
            Command::Paste { text } => match self.focused() {
                Some(View::Terminal(id)) => self
                    .terminals
                    .get_mut(&id)
                    .context("Terminal missing")?
                    .paste(&text)?,
                Some(View::Editor(id)) => self.edit(id, &text)?,
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
                        let v = self.views.get_mut(&id).unwrap();
                        let text = &self.documents[&v.document].text;
                        v.manual_scroll = false;
                        let cursor = at_line_col(text, v.top + row, v.left + col.saturating_sub(6));
                        if shift {
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
                    _ => {}
                }
            }
            Command::Scroll { pane, delta } => match self.layout.view(pane).cloned() {
                Some(View::Editor(id)) => {
                    let v = self.views.get_mut(&id).unwrap();
                    v.top = (v.top as i64 - delta as i64).max(0) as usize;
                    v.manual_scroll = true;
                }
                Some(View::Terminal(id)) => self.terminals.get_mut(&id).unwrap().scroll(delta),
                _ => {}
            },
            Command::Open { path } => {
                let path = if path.is_absolute() {
                    path
                } else {
                    self.root.join(path)
                };
                if path.is_dir() {
                    self.browse(path)?;
                } else {
                    let path = path.canonicalize()?;
                    let doc = if let Some((id, _)) = self
                        .documents
                        .iter()
                        .find(|(_, d)| d.path.as_ref() == Some(&path))
                    {
                        *id
                    } else {
                        let id = self.id();
                        self.documents.insert(id, Document::open(&path)?);
                        id
                    };
                    let pane = if self.active_editor().is_some() {
                        self.focus
                    } else {
                        let _ = self.editor_target();
                        self.focus
                    };
                    let existing=self.layout.pane_mut(pane).and_then(|(tabs,_)|tabs.iter().position(|v|matches!(v,View::Editor(id) if self.views.get(id).map(|v|v.document)==Some(doc))));
                    if let Some(index) = existing {
                        *self.layout.pane_mut(pane).unwrap().1 = index;
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
                    self.status = format!("Opened {}", path.display());
                }
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
                let destination = if let Command::SaveAs { path } = cmd {
                    Some(if path.is_absolute() {
                        path
                    } else {
                        self.root.join(path)
                    })
                } else {
                    None
                };
                let id = self.active_editor().context("Focus an editor to save")?;
                let doc = self.views[&id].document;
                self.documents
                    .get_mut(&doc)
                    .unwrap()
                    .save(destination.as_deref())?;
                self.status = "Saved".into();
                self.refresh();
            }
            Command::CloseDocument { force } => {
                let id = self.active_editor().context("Focus an editor first")?;
                let doc = self.views[&id].document;
                if self.documents[&doc].dirty() && !force {
                    bail!("Unsaved document. Save it or explicitly discard it from the command palette");
                }
                let affected: Vec<_> = self
                    .views
                    .iter()
                    .filter(|(_, v)| v.document == doc)
                    .map(|(id, _)| *id)
                    .collect();
                let replacement = self.id();
                self.documents.insert(replacement, Document::scratch());
                for id in affected {
                    self.views.insert(
                        id,
                        EditorView {
                            document: replacement,
                            ..Default::default()
                        },
                    );
                }
                self.documents.remove(&doc);
            }
            Command::Undo | Command::Redo => {
                let id = self.active_editor().context("Focus an editor first")?;
                let view = self.views[&id].clone();
                let d = self.documents.get_mut(&view.document).unwrap();
                let cursor = if matches!(cmd, Command::Undo) {
                    d.undo(view.cursor)
                } else {
                    d.redo(view.cursor)
                };
                if let Some(cursor) = cursor {
                    self.clamp_views(view.document);
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
                    self.focus = pane;
                }
            }
            Command::FocusNext => {
                let panes = self.layout.panes();
                let current = panes.iter().position(|p| *p == self.focus).unwrap_or(0);
                self.focus = panes[(current + 1) % panes.len()];
            }
            Command::SwitchTab { pane, index } => {
                if let Some((tabs, active)) = self.layout.pane_mut(pane) {
                    if index < tabs.len() {
                        *active = index;
                        self.focus = pane;
                    }
                }
            }
            Command::NextTab => {
                if let Some((tabs, active)) = self.layout.pane_mut(self.focus) {
                    *active = (*active + 1) % tabs.len();
                }
            }
            Command::NewTerminal => {
                let id = self.new_terminal()?;
                self.add_tab(View::Terminal(id));
            }
            Command::TerminateTerminal => {
                let Some(View::Terminal(id)) = self.focused() else {
                    bail!("Focus a terminal first")
                };
                self.terminals.remove(&id);
                self.remove_terminal_views(id);
            }
            Command::AddView { kind } => {
                let v = self.create_view(&kind)?;
                self.add_tab(v);
            }
            Command::Split { axis, kind } => {
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
                if self.layout.remove(self.focus) {
                    self.focus = self.layout.panes()[0];
                } else {
                    bail!("Cannot close the final pane");
                }
            }
            Command::MovePane { target } => {
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
                self.git_action(vec!["add".into(), "--".into(), path], false);
            }
            Command::GitUnstage { path } => {
                self.git_action(
                    vec!["reset".into(), "HEAD".into(), "--".into(), path],
                    false,
                );
            }
            Command::GitDiff { path } => {
                self.git_action(vec!["diff".into(), "HEAD".into(), "--".into(), path], true);
            }
            Command::GitCommit { message } => {
                if message.trim().is_empty() {
                    bail!("Commit message is empty");
                }
                self.git_action(vec!["commit".into(), "-m".into(), message], false);
            }
            Command::Quit { force } => {
                if self.dirty() && !force {
                    bail!("Unsaved documents. Save them or explicitly discard and quit");
                }
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
        let _ = self.services.tx.send(Job::Git(
            self.root.clone(),
            vec![
                "status".into(),
                "--porcelain=v1".into(),
                "--untracked-files=all".into(),
                "-z".into(),
            ],
            false,
        ));
    }
    fn git_action(&mut self, args: Vec<String>, diff: bool) {
        let _ = self
            .services
            .tx
            .send(Job::Git(self.root.clone(), args, diff));
        if !diff {
            self.refresh_git();
        }
        self.status = "Running Git…".into();
    }
    pub fn poll(&mut self) {
        while let Ok(reply) = self.services.rx.try_recv() {
            match reply {
                Reply::Files(path, Ok(entries)) if path == self.browser => {
                    self.files = entries;
                    self.selected = self.selected.min(self.files.len().saturating_sub(1));
                }
                Reply::Files(_, Err(e)) => self.status = e,
                Reply::Git(Ok(entries)) => {
                    self.git = entries;
                    self.git_selected = self.git_selected.min(self.git.len().saturating_sub(1));
                }
                Reply::Git(Err(e)) | Reply::Error(e) => self.status = e,
                Reply::Message(message) => self.status = message,
                Reply::Output(text) => {
                    let doc = self.id();
                    let mut d = Document::scratch();
                    d.text = text;
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
                    self.status = "Git diff (save as a new file if needed)".into();
                }
                _ => {}
            }
        }
    }
    fn new_terminal(&mut self) -> Result<u64> {
        let id = self.id();
        self.terminals
            .insert(id, TerminalSession::spawn(&self.root, None, 24, 80)?);
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
            tabs.retain(|v| *v != View::Terminal(id));
            if tabs.is_empty() {
                tabs.push(View::Files);
            }
            *active = (*active).min(tabs.len() - 1);
        }
    }
    fn preset(&mut self, name: &str) -> Result<()> {
        let editor = self.editor_target();
        let terminal = self.terminals.keys().next().copied();
        match name {
            "minimal" => {
                let id = self.id();
                self.layout = Node::pane(id, View::Editor(editor));
            }
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
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
            })
            .join("slate/layouts.toml")
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
            v.anchor = None;
        }
    }
    fn edit(&mut self, id: u64, value: &str) -> Result<()> {
        let v = self.views[&id].clone();
        let anchor = v.anchor.unwrap_or(v.cursor);
        let start = v.cursor.min(anchor);
        let end = v.cursor.max(anchor);
        self.documents
            .get_mut(&v.document)
            .unwrap()
            .replace(start, end, value, v.cursor)?;
        for other in self.views.values_mut().filter(|o| o.document == v.document) {
            other.cursor = if other.cursor <= start {
                other.cursor
            } else if other.cursor >= end {
                other.cursor - end + start + value.len()
            } else {
                start + value.len()
            };
            other.anchor = None;
        }
        self.views.get_mut(&id).unwrap().cursor = start + value.len();
        self.views.get_mut(&id).unwrap().manual_scroll = false;
        Ok(())
    }
    fn key(&mut self, k: Key) -> Result<()> {
        match k.key.as_str() {
            "F6" => return self.execute(Command::FocusNext),
            "F7" => return self.execute(Command::NextTab),
            "F8" => return self.execute(Command::NewTerminal),
            "F9" => {
                return self.execute(Command::Split {
                    axis: if k.shift {
                        Axis::Vertical
                    } else {
                        Axis::Horizontal
                    },
                    kind: None,
                })
            }
            _ => {}
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
                            if let Some(e) = self.git.get(*selected) {
                                return self.execute(Command::GitDiff {
                                    path: e.path.clone(),
                                });
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
                let lower = k.key.to_lowercase();
                if k.ctrl {
                    match lower.as_str() {
                        "s" => return self.execute(Command::Save),
                        "z" => {
                            return self.execute(if k.shift {
                                Command::Redo
                            } else {
                                Command::Undo
                            })
                        }
                        "y" => return self.execute(Command::Redo),
                        "a" => return self.execute(Command::SelectAll),
                        "c" => return self.execute(Command::Copy),
                        "x" => return self.execute(Command::Cut),
                        "v" => return self.edit(id, &self.clipboard.clone()),
                        _ => {}
                    }
                }
                let v = self.views[&id].clone();
                let text = &self.documents[&v.document].text;
                let (row, col) = line_col(text, v.cursor);
                let movement = match k.key.as_str() {
                    "Left" => Some(previous(text, v.cursor)),
                    "Right" => Some(next(text, v.cursor)),
                    "Up" => Some(at_line_col(text, row.saturating_sub(1), col)),
                    "Down" => Some(at_line_col(text, row + 1, col)),
                    "Home" => Some(if k.ctrl {
                        0
                    } else {
                        line_start(text, v.cursor)
                    }),
                    "End" => Some(if k.ctrl {
                        text.len()
                    } else {
                        line_end(text, v.cursor)
                    }),
                    "PageUp" => Some(at_line_col(
                        text,
                        row.saturating_sub(v.rows.max(1) as usize),
                        col,
                    )),
                    "PageDown" => Some(at_line_col(text, row + v.rows.max(1) as usize, col)),
                    _ => None,
                };
                if let Some(cursor) = movement {
                    let view = self.views.get_mut(&id).unwrap();
                    if k.shift {
                        view.anchor.get_or_insert(view.cursor);
                    } else {
                        view.anchor = None;
                    }
                    view.cursor = cursor;
                    view.manual_scroll = false;
                    return Ok(());
                }
                match k.key.as_str() {
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
                        let value = if text.contains("\r\n") { "\r\n" } else { "\n" };
                        self.edit(id, value)?;
                    }
                    "Tab" => self.edit(id, "\t")?,
                    "Escape" => self.views.get_mut(&id).unwrap().anchor = None,
                    _ if !k.ctrl && !k.alt && !k.text.is_empty() => self.edit(id, &k.text)?,
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
        self.poll();
        let cw = cell_width.max(1);
        let ch = cell_height.max(1);
        let area = Rect {
            x: 0,
            y: 0,
            width,
            height,
        };
        let (placements, handles) = if width / cw < 70 || height / ch < 10 {
            (
                vec![layout::Placement {
                    id: self.focus,
                    rect: area,
                }],
                vec![],
            )
        } else {
            self.layout.arrange(area, gap)
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
            let (kind, screen, selected) = match view {
                View::Editor(view) => {
                    let screen = self.editor_screen(view, rows, cols);
                    ("editor", Some(screen), 0)
                }
                View::Terminal(term) => {
                    let screen = self.terminals.get_mut(&term).map(|t| {
                        let _ = t.resize(rows, cols);
                        t.screen()
                    });
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
                screen,
                selected,
            });
        }
        Snapshot {
            panes,
            handles,
            focus: self.focus,
            files: self.files.clone(),
            git: self.git.clone(),
            browser: self.browser.to_string_lossy().into_owned(),
            status: self.status.clone(),
            dirty: self.dirty(),
            quit: self.quit,
            clipboard: self.clipboard.clone(),
            layouts: self.presets.keys().cloned().collect(),
        }
    }
    fn editor_screen(&mut self, id: u64, rows: u16, cols: u16) -> Screen {
        let v = self.views.get_mut(&id).unwrap();
        v.rows = rows;
        v.cols = cols;
        let text = &self.documents[&v.document].text;
        let (row, col) = line_col(text, v.cursor);
        let gutter = 6usize.min(cols as usize);
        if !v.manual_scroll {
            if row < v.top {
                v.top = row;
            } else if row >= v.top + rows as usize {
                v.top = row + 1 - rows as usize;
            }
        }
        let usable = (cols as usize).saturating_sub(gutter).max(1);
        if col < v.left {
            v.left = col;
        } else if col >= v.left + usable {
            v.left = col + 1 - usable;
        }
        let selection = v.anchor.map(|a| (a.min(v.cursor), a.max(v.cursor)));
        let mut cells = vec![vec![Cell::text(" "); cols as usize]; rows as usize];
        let start = at_line_col(text, v.top, 0);
        let mut byte = start;
        for (y, line) in text[start..].split('\n').take(rows as usize).enumerate() {
            let number = format!("{:>4}  ", v.top + y + 1);
            for (x, c) in number.chars().take(gutter).enumerate() {
                cells[y][x] = Cell::text(c.to_string());
                cells[y][x].fg = "#68778d".into();
            }
            let mut x = 0;
            for (i, g) in line.grapheme_indices(true) {
                if g == "\r" {
                    continue;
                }
                let width = if g == "\t" {
                    4 - x % 4
                } else {
                    UnicodeWidthStr::width(g).max(1)
                };
                for offset in 0..width {
                    let column = x + offset;
                    if column >= v.left && column - v.left + gutter < cols as usize {
                        let mut c = Cell::text(if g == "\t" || offset > 0 {
                            " ".to_string()
                        } else {
                            g.chars()
                                .map(|c| if c.is_control() { '�' } else { c })
                                .collect()
                        });
                        c.wide = width == 2 && g != "\t" && offset == 0;
                        c.continuation = width == 2 && g != "\t" && offset == 1;
                        if selection
                            .map(|(a, b)| byte + i >= a && byte + i < b)
                            .unwrap_or(false)
                        {
                            c.bg = "#425b78".into();
                        }
                        cells[y][column - v.left + gutter] = c;
                    }
                }
                x += width;
            }
            byte += line.len() + 1;
        }
        Screen {
            cells,
            cursor: if row >= v.top && row < v.top + rows as usize && col >= v.left {
                Some((
                    (row - v.top) as u16,
                    ((col - v.left) + gutter).min(cols as usize - 1) as u16,
                ))
            } else {
                None
            },
            rows,
            cols,
        }
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

pub const COMMAND_HELP:&str="open PATH | save | save-as PATH | new | close | discard-document | quit | discard-quit | undo | redo | copy | cut | paste | select-all | split-right | split-down | split-terminal-right | split-terminal-down | terminal | terminate-terminal | files | git | editor | close-pane | move-pane ID | next-pane | next-tab | resize ID RATIO | preset development|minimal|bottom_terminal | layout-save NAME | layout-load NAME | refresh | stage | unstage | diff | commit MESSAGE";
impl App {
    pub fn command_line(&mut self, line: &str) {
        let (verb, args) = line.trim().split_once(' ').unwrap_or((line.trim(), ""));
        let args = args.trim();
        let selected = || self.selected_path().unwrap_or_default();
        let cmd = match verb {
            "open" => Some(Command::Open { path: args.into() }),
            "save" => Some(Command::Save),
            "save-as" => Some(Command::SaveAs { path: args.into() }),
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
            "diff" => Some(Command::GitDiff { path: selected() }),
            "commit" => Some(Command::GitCommit {
                message: args.into(),
            }),
            _ => None,
        };
        if let Some(cmd) = cmd {
            self.dispatch(cmd);
        } else {
            self.status = format!("Unknown/incomplete command. {COMMAND_HELP}");
        }
    }
}
