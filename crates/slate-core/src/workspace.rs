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
    /// Breakpoint lines (zero-based) by file.
    #[serde(default)]
    pub breakpoints: BTreeMap<PathBuf, Vec<usize>>,
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
            if v.cursor > d.len()
                || !d.is_char_boundary(v.cursor)
                || v.anchor
                    .is_some_and(|a| a > d.len() || !d.is_char_boundary(a))
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
impl Workspace {
    /// Rebuild what can be trusted from a checkpoint that failed validation:
    /// every buffer that still parses and fits the limits, views clamped to
    /// their text, and the layout (or a plain one when the saved layout is
    /// unusable). Returns `None` when nothing worth keeping is left.
    pub fn salvage(bytes: &[u8], root: &Path) -> Option<Self> {
        use serde_json::Value;
        let value: Value = serde_json::from_slice(bytes).ok()?;
        if value.get("version")?.as_u64()? != 1
            || value
                .get("root")
                .and_then(|r| serde_json::from_value::<PathBuf>(r.clone()).ok())?
                != root
        {
            return None;
        }
        let entries = |key: &str| -> Vec<(u64, Value)> {
            value
                .get(key)
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| Some((k.parse().ok()?, v.clone())))
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut documents = BTreeMap::new();
        for (id, v) in entries("documents") {
            if documents.len() == 128 {
                break;
            }
            if let Ok(d) = serde_json::from_value::<Document>(v) {
                if d.valid_recovery() {
                    documents.insert(id, d);
                }
            }
        }
        if documents.is_empty() {
            return None;
        }
        let mut views = BTreeMap::new();
        for (id, v) in entries("views") {
            let Ok(mut view) = serde_json::from_value::<EditorView>(v) else {
                continue;
            };
            let Some(d) = documents.get(&view.document) else {
                continue;
            };
            if views.len() == 512 {
                break;
            }
            if !d.reference {
                view.cursor = d.floor_boundary(view.cursor.min(d.len()));
                view.anchor = view.anchor.map(|a| d.floor_boundary(a.min(d.len())));
            }
            views.insert(id, view);
        }
        let terminals: BTreeMap<u64, PathBuf> = entries("terminals")
            .into_iter()
            .filter_map(|(id, v)| Some((id, serde_json::from_value(v).ok()?)))
            .take(128)
            .collect();
        let browser = value
            .get("browser")
            .and_then(|b| serde_json::from_value(b.clone()).ok())
            .unwrap_or_else(|| root.to_path_buf());
        let mut ids = documents
            .keys()
            .chain(views.keys())
            .chain(terminals.keys())
            .copied()
            .max()
            .unwrap_or(0)
            .max(value.get("ids").and_then(Value::as_u64).unwrap_or(0))
            .min(u64::MAX - 20_000);
        let mut w = Workspace {
            breakpoints: value
                .get("breakpoints")
                .and_then(|b| serde_json::from_value(b.clone()).ok())
                .unwrap_or_default(),
            version: 1,
            root: root.to_path_buf(),
            documents,
            views,
            terminals,
            layout: Node::pane(0, View::Files),
            focus: 0,
            browser,
            ids,
        };
        let saved = value
            .get("layout")
            .and_then(|l| serde_json::from_value::<Node>(l.clone()).ok());
        let focus = value.get("focus").and_then(Value::as_u64);
        if let Some(layout) = saved {
            w.layout = layout;
            w.focus = focus.unwrap_or(0);
            w.ids = w.ids.max(w.layout.panes().into_iter().max().unwrap_or(0));
            if w.validate(root).is_ok() {
                return Some(w);
            }
            if w.layout.validate() && !w.layout.panes().contains(&w.focus) {
                w.focus = w.layout.panes()[0];
                if w.validate(root).is_ok() {
                    return Some(w);
                }
            }
        }
        // A plain layout: one pane with the first editor of each document.
        // The application moves any further views and shells into it.
        let mut tabs = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for (id, v) in &w.views {
            if seen.insert(v.document) && tabs.len() < crate::layout::MAX_TABS {
                tabs.push(View::Editor(*id));
            }
        }
        if tabs.is_empty() {
            ids += 1;
            w.views.insert(
                ids,
                EditorView {
                    document: *w.documents.keys().next()?,
                    ..Default::default()
                },
            );
            tabs.push(View::Editor(ids));
        }
        ids = ids.max(w.ids) + 1;
        w.layout = Node::Pane {
            id: ids,
            tabs,
            active: 0,
        };
        w.focus = ids;
        w.ids = ids;
        w.validate(root).ok()?;
        Some(w)
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
        Ok(self.load_tolerant(root)?.map(|(w, _)| w))
    }
    /// Load the checkpoint. One that fails validation is salvaged when any
    /// buffer can be kept; the original file is then preserved next to it and
    /// its path returned so the user can inspect it.
    pub fn load_tolerant(&self, root: &Path) -> Result<Option<(Workspace, Option<PathBuf>)>> {
        if !self.path.exists() {
            return Ok(None);
        }
        if fs::metadata(&self.path)?.len() > 128 * 1024 * 1024 {
            bail!("Recovery checkpoint is too large");
        }
        let bytes = fs::read(&self.path)?;
        let error = match serde_json::from_slice::<Workspace>(&bytes) {
            Ok(w) => match w.validate(root) {
                Ok(()) => return Ok(Some((w, None))),
                Err(e) => e,
            },
            Err(e) => e.into(),
        };
        let Some(w) = Workspace::salvage(&bytes, root) else {
            return Err(error);
        };
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let kept = self
            .path
            .with_file_name(format!("session.damaged-{stamp}.json"));
        fs::copy(&self.path, &kept)?;
        Ok(Some((w, Some(kept))))
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

use crate::{services::IoJob, App, Command};

impl App {
    pub fn enable_workspace(&mut self, recover: bool) -> Result<()> {
        let store = WorkspaceStore::acquire(&self.recovery_key)?;
        self.attach_workspace(store, recover)
    }
    pub fn attach_workspace(&mut self, store: WorkspaceStore, recover: bool) -> Result<()> {
        let requested = self.documents.values().find_map(|d| d.path.clone());
        // A fresh start replaces the checkpoint; keep the old one (which may
        // hold unsaved work from a crash) beside it.
        if !recover && store.path.exists() {
            let previous = store.path.with_file_name("session.previous.json");
            let _ = fs::rename(&store.path, previous);
        }
        if recover {
            if let Some((mut w, damaged)) = store.load_tolerant(&self.root)? {
                // Content versions are not stored; dirty means text differs.
                for d in w.documents.values_mut() {
                    d.restore_versions();
                }
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
                        terminals.insert(*id, self.spawn_shell(cwd)?);
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
                self.restore_breakpoints(w.breakpoints);
                self.browser = if w.browser.is_dir() {
                    w.browser
                } else {
                    self.root.clone()
                };
                let docs = self.documents.keys().copied().collect::<Vec<_>>();
                for doc in docs {
                    self.clamp_views(doc);
                }
                // Salvaged checkpoints may have lost tabs for some views.
                self.adopt_orphans();
                self.status = match damaged {
                    None => format!("Restored workspace · {recovered} recovered unsaved buffers · fresh terminal shells"),
                    Some(kept) => format!(
                        "Recovered {recovered} unsaved buffers from a damaged checkpoint · original kept at {}",
                        kept.display()
                    ),
                };
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
        // Output and inspection documents (task output, the debugger, Help,
        // diffs) describe a moment that has passed; they are not restored.
        let transient: std::collections::BTreeSet<u64> = self
            .documents
            .iter()
            .filter(|(_, d)| d.label.is_some())
            .map(|(id, _)| *id)
            .collect();
        let keep_all = transient.len() == self.documents.len();
        let dropped = |doc: &u64| !keep_all && transient.contains(doc);
        let views: BTreeMap<u64, EditorView> = self
            .views
            .iter()
            .filter(|(_, v)| !dropped(&v.document))
            .map(|(id, v)| (*id, v.clone()))
            .collect();
        let mut layout = self.layout.clone();
        for pane in layout.panes() {
            if let Some((tabs, active)) = layout.pane_mut(pane) {
                let before = tabs.len();
                tabs.retain(|t| !matches!(t, View::Editor(id) if !views.contains_key(id)));
                if tabs.is_empty() {
                    tabs.push(View::Files);
                }
                if tabs.len() != before {
                    *active = (*active).min(tabs.len() - 1);
                }
            }
        }
        Workspace {
            version: 1,
            root: self.root.clone(),
            documents: self
                .documents
                .iter()
                .filter(|(id, _)| !dropped(id))
                .map(|(id, d)| (*id, d.recovery_copy()))
                .collect(),
            views,
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
            layout,
            focus: self.focus,
            browser: self.browser.clone(),
            ids: self.ids,
            breakpoints: self.saved_breakpoints(),
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
        self.saved_breakpoints().hash(&mut hasher);
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
        let deadline = Instant::now() + std::time::Duration::from_secs(15);
        loop {
            // A bounded reply queue must keep draining while the I/O barrier
            // waits, otherwise its worker can block before reaching Flush.
            self.poll();
            match rx.recv_timeout(std::time::Duration::from_millis(5)) {
                Ok(()) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    anyhow::bail!("Recovery worker stopped")
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) if Instant::now() >= deadline => {
                    anyhow::bail!("Recovery flush timed out")
                }
                Err(_) => {}
            }
        }
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
        // Tasks run in their own process groups, so nothing else stops them.
        self.kill_tasks();
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
    fn flushing_drains_a_full_reply_queue_before_waiting_for_the_barrier() {
        let _serial = crate::paths::TEST_ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
        std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
        let mut app = App::new(dir.path()).unwrap();
        // Use idle workers so no startup reply can race the saturation fixture.
        app.services = crate::services::Services::new(app.events.clone());
        let sender = app.services.reply_sender();
        for _ in 0..256 {
            sender
                .send(crate::services::Reply::Error("fixture".into()))
                .unwrap();
        }
        let path = dir.path().join("barrier.txt");
        std::fs::write(&path, "barrier").unwrap();
        app.services.io.send(IoJob::Open(999, path)).unwrap();
        app.flush_workspace().unwrap();
    }

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
