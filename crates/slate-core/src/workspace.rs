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
            // Clean files are stored as references; their views are clamped
            // after the current file contents are read.
            if d.reference {
                continue;
            }
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
impl Drop for WorkspaceStore {
    fn drop(&mut self) {
        // A concurrently forked worker can briefly inherit this descriptor.
        // Release the lock explicitly rather than waiting for every inherited
        // descriptor to close, so an immediate restart can acquire the store.
        let _ = FileExt::unlock(&self._lock);
    }
}
impl WorkspaceStore {
    pub fn acquire(root: &Path) -> Result<Self> {
        let state = crate::paths::state_dir();
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
        // A killed instance can leave a partially written temporary checkpoint.
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().starts_with(".tmp") {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
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
        let store = WorkspaceStore::acquire(&self.recovery_key)?;
        self.attach_workspace(store, recover)
    }
    pub fn attach_workspace(&mut self, store: WorkspaceStore, recover: bool) -> Result<()> {
        let requested = self.documents.values().find_map(|d| d.path.clone());
        if recover {
            if let Some(mut w) = store.load(&self.root)? {
                let recovered = w
                    .documents
                    .values()
                    .filter(|d| !d.reference && d.dirty())
                    .count();
                // Clean buffers follow current disk contents. Dirty buffers retain their original save baseline.
                for d in w.documents.values_mut() {
                    if d.reference {
                        let read_only = d.read_only;
                        if let Some(path) = d.path.clone() {
                            *d = Document::open(&path)
                                .or_else(|_| Document::new_file(&path))
                                .unwrap_or_else(|_| Document::scratch());
                            d.read_only = read_only;
                        }
                    } else if !d.dirty() {
                        if let Some(path) = &d.path {
                            match Document::open(path) {
                                Ok(current) => *d = current,
                                Err(_) => d.mark_unavailable(),
                            }
                        }
                    }
                }
                // Prepare recovered shells before replacing the live model. A
                // spawn failure must leave a complete, usable current workspace.
                let mut terminals = BTreeMap::new();
                if !self.editor_only {
                    for (id, cwd) in &w.terminals {
                        let mut terminal = TerminalSession::spawn(
                            if cwd.is_dir() { cwd } else { &self.root },
                            None,
                            24,
                            80,
                        )?;
                        terminal.set_events(self.events.clone());
                        terminals.insert(*id, terminal);
                    }
                }
                self.documents = w.documents;
                self.views = w.views;
                self.terminals = terminals;
                self.deferred_terminals = if self.editor_only {
                    w.terminals
                } else {
                    BTreeMap::new()
                };
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
        self.select_editor_pane();
        if !self.editor_only {
            self.ensure_terminals()?;
        }
        for terminal in self.terminals.values_mut() {
            terminal.set_events(self.events.clone());
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
                .map(|(id, d)| (*id, d.recovery_copy()))
                .collect(),
            views: self.views.clone(),
            terminals: self
                .terminals
                .iter()
                .map(|(id, t)| (*id, t.cwd()))
                .chain(
                    self.deferred_terminals
                        .iter()
                        .map(|(id, cwd)| (*id, cwd.clone())),
                )
                .collect(),
            layout: self.layout.clone(),
            focus: self.focus,
            browser: self.browser.clone(),
            ids: self.ids,
        }
    }
    /// A cheap fingerprint of everything a checkpoint records, so unchanged
    /// state (pointer motion, idle redraws) never rewrites the recovery file.
    fn checkpoint_fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for (id, d) in &self.documents {
            (id, d.generation, d.dirty(), &d.path, d.read_only).hash(&mut hasher);
        }
        serde_json::to_string(&(
            &self.views,
            &self.layout,
            self.focus,
            &self.browser,
            self.ids,
        ))
        .unwrap_or_default()
        .hash(&mut hasher);
        for (id, t) in &self.terminals {
            (id, t.cwd()).hash(&mut hasher);
        }
        self.deferred_terminals.hash(&mut hasher);
        hasher.finish()
    }
    pub(super) fn checkpoint(&mut self) {
        let fingerprint = self.checkpoint_fingerprint();
        if fingerprint == self.checkpoint_key {
            self.workspace_dirty = false;
            self.checkpoint_at = Instant::now();
            return;
        }
        if let Some(store) = &self.store {
            let result = self
                .services
                .io
                .send(IoJob::Checkpoint(store.path.clone(), self.workspace()));
            if result.is_ok() {
                self.workspace_dirty = false;
                self.checkpoint_key = fingerprint;
            } else {
                self.status = "Recovery worker is unavailable".into();
            }
        }
        self.checkpoint_at = Instant::now();
    }
    pub fn flush_workspace(&mut self) -> Result<()> {
        let (tx, rx) = std::sync::mpsc::sync_channel(0);
        self.services
            .io
            .send(IoJob::Flush(tx))
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releasing_the_store_unlocks_even_with_an_inherited_descriptor() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = WorkspaceStore::acquire_in(root.path(), state.path()).unwrap();
        let inherited = store._lock.try_clone().unwrap();
        assert!(WorkspaceStore::acquire_in(root.path(), state.path()).is_err());
        drop(store);
        let reopened = WorkspaceStore::acquire_in(root.path(), state.path()).unwrap();
        drop(inherited);
        assert!(WorkspaceStore::acquire_in(root.path(), state.path()).is_err());
        drop(reopened);
    }
}
