use crate::{
    document::Document,
    layout::{Node, View},
    EditorView,
};
use anyhow::{bail, Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Serialize, Deserialize)]
pub struct Workspace {
    pub version: u32,
    pub root: PathBuf,
    pub documents: BTreeMap<u64, Document>,
    pub views: BTreeMap<u64, EditorView>,
    pub terminals: BTreeMap<u64, PathBuf>,
    pub layout: Node,
    pub focus: u64,
    pub browser: PathBuf,
    pub ids: u64,
}
impl Workspace {
    pub fn validate(&self, root: &Path) -> Result<()> {
        if self.version != 1
            || self.root != root
            || !self.layout.validate()
            || self.documents.is_empty()
            || self.documents.len() > 128
            || self.views.len() > 512
            || self.terminals.len() > 128
            || !self.layout.panes().contains(&self.focus)
        {
            bail!("Invalid workspace checkpoint");
        }
        for d in self.documents.values() {
            if !d.valid_recovery() {
                bail!("Recovery buffer exceeds file limit");
            }
        }
        for v in self.views.values() {
            let d = self
                .documents
                .get(&v.document)
                .context("Recovery view has no document")?;
            if v.cursor > d.text.len()
                || !d.text.is_char_boundary(v.cursor)
                || v.anchor
                    .is_some_and(|a| a > d.text.len() || !d.text.is_char_boundary(a))
            {
                bail!("Invalid recovery cursor");
            }
        }
        let mut max_id = 0;
        fn check(n: &Node, w: &Workspace, max_id: &mut u64) -> Result<()> {
            match n {
                Node::Pane { id, tabs, .. } => {
                    *max_id = (*max_id).max(*id);
                    for tab in tabs {
                        match tab {
                            View::Editor(id) if !w.views.contains_key(id) => {
                                bail!("Missing recovery editor")
                            }
                            View::Terminal(id) if !w.terminals.contains_key(id) => {
                                bail!("Missing recovery terminal")
                            }
                            _ => {}
                        }
                    }
                }
                Node::Split {
                    id, first, second, ..
                } => {
                    *max_id = (*max_id).max(*id);
                    check(first, w, max_id)?;
                    check(second, w, max_id)?;
                }
            }
            Ok(())
        }
        check(&self.layout, self, &mut max_id)?;
        for id in self
            .documents
            .keys()
            .chain(self.views.keys())
            .chain(self.terminals.keys())
        {
            max_id = max_id.max(*id);
        }
        if self.ids < max_id || self.ids > u64::MAX - 10_000 {
            bail!("Invalid recovery ID counter");
        }
        Ok(())
    }
}
pub struct WorkspaceStore {
    pub path: PathBuf,
    _lock: File,
}
impl WorkspaceStore {
    pub fn acquire(root: &Path) -> Result<Self> {
        let state = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state")
            });
        Self::acquire_in(root, &state)
    }
    pub fn acquire_in(root: &Path, state: &Path) -> Result<Self> {
        let hash = format!("{:x}", Sha256::digest(root.as_os_str().as_encoded_bytes()));
        let dir = state.join("slate/workspaces").join(hash);
        fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
        }
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(dir.join("lock"))?;
        lock.try_lock_exclusive().context(
            "Workspace is already open; this instance will not replace its recovery state",
        )?;
        Ok(Self {
            path: dir.join("session.json"),
            _lock: lock,
        })
    }
    pub fn load(&self, root: &Path) -> Result<Option<Workspace>> {
        if !self.path.exists() {
            return Ok(None);
        }
        if fs::metadata(&self.path)?.len() > 128 * 1024 * 1024 {
            bail!("Recovery checkpoint is too large");
        }
        let w: Workspace = serde_json::from_slice(&fs::read(&self.path)?)?;
        w.validate(root)?;
        Ok(Some(w))
    }
}
pub fn write_checkpoint(path: &Path, workspace: &Workspace) -> Result<()> {
    let mut file =
        tempfile::NamedTempFile::new_in(path.parent().context("Recovery directory missing")?)?;
    struct LimitedWriter<'a> {
        writer: &'a mut File,
        remaining: usize,
    }
    impl Write for LimitedWriter<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.remaining {
                return Err(std::io::Error::other("Recovery checkpoint exceeds 128 MiB"));
            }
            let n = self.writer.write(bytes)?;
            self.remaining -= n;
            Ok(n)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.writer.flush()
        }
    }
    serde_json::to_writer(
        LimitedWriter {
            writer: file.as_file_mut(),
            remaining: 128 * 1024 * 1024,
        },
        workspace,
    )?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    #[cfg(unix)]
    File::open(path.parent().unwrap())?.sync_all()?;
    Ok(())
}

use crate::{services::IoJob, terminal::TerminalSession, App, Command};

impl App {
    pub fn enable_workspace(&mut self, recover: bool) -> Result<()> {
        let store = WorkspaceStore::acquire(&self.root)?;
        self.attach_workspace(store, recover)
    }
    pub fn attach_workspace(&mut self, store: WorkspaceStore, recover: bool) -> Result<()> {
        let requested = self.documents.values().find_map(|d| d.path.clone());
        if recover {
            if let Some(mut w) = store.load(&self.root)? {
                let recovered = w.documents.values().filter(|d| d.dirty()).count();
                // Clean buffers follow current disk contents. Dirty buffers retain their original save baseline.
                for d in w.documents.values_mut() {
                    if !d.dirty() {
                        if let Some(path) = &d.path {
                            match Document::open(path) {
                                Ok(current) => *d = current,
                                Err(_) => d.mark_unavailable(),
                            }
                        }
                    }
                }
                let mut terminals = BTreeMap::new();
                for (id, cwd) in &w.terminals {
                    terminals.insert(
                        *id,
                        TerminalSession::spawn(
                            if cwd.is_dir() { cwd } else { &self.root },
                            None,
                            24,
                            80,
                        )?,
                    );
                }
                self.documents = w.documents;
                self.views = w.views;
                self.terminals = terminals;
                self.layout = w.layout;
                self.focus = w.focus;
                self.ids = w.ids;
                self.browser = if w.browser.is_dir() {
                    w.browser
                } else {
                    self.root.clone()
                };
                let docs = self.documents.keys().copied().collect::<Vec<_>>();
                for doc in docs {
                    self.clamp_views(doc);
                }
                self.status=format!("Restored workspace · {recovered} recovered unsaved buffers · fresh terminal shells");
                if let Some(path) = requested {
                    self.dispatch(Command::Open { path });
                }
            }
        }
        self.store = Some(store);
        self.workspace_dirty = true;
        self.refresh();
        Ok(())
    }
    pub(super) fn workspace(&self) -> Workspace {
        Workspace {
            version: 1,
            root: self.root.clone(),
            documents: self
                .documents
                .iter()
                .map(|(id, d)| (*id, d.checkpoint()))
                .collect(),
            views: self.views.clone(),
            terminals: self
                .terminals
                .iter()
                .map(|(id, t)| (*id, t.cwd()))
                .collect(),
            layout: self.layout.clone(),
            focus: self.focus,
            browser: self.browser.clone(),
            ids: self.ids,
        }
    }
    pub(super) fn checkpoint(&mut self) {
        if let Some(store) = &self.store {
            let result = self
                .services
                .io
                .send(IoJob::Checkpoint(store.path.clone(), self.workspace()));
            if result.is_ok() {
                self.workspace_dirty = false;
            } else {
                self.status = "Recovery worker is unavailable".into();
            }
        }
        self.checkpoint_at = Instant::now();
    }
    pub fn flush_workspace(&mut self) -> Result<()> {
        let (tx, rx) = std::sync::mpsc::sync_channel(0);
        self.services.io.send(IoJob::Flush(tx))?;
        rx.recv()?;
        self.poll();
        if let Some(store) = &self.store {
            write_checkpoint(&store.path, &self.workspace())?;
        }
        self.workspace_dirty = false;
        Ok(())
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if self.store.is_some() {
            if let Err(e) = self.flush_workspace() {
                eprintln!("Could not persist Slate workspace: {e:#}");
            }
        }
    }
}
