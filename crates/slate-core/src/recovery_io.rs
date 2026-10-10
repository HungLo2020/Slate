//! Atomic checkpoint metadata and separately bounded, verified rope payloads.
//! Small buffers stay inline for compatibility; external payloads require schema 2.
use crate::{
    document::{Document, MAX_FILE_BYTES},
    workspace::Workspace,
};
use anyhow::{bail, Context, Result};
use ropey::Rope;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

pub(crate) const METADATA_LIMIT: u64 = 128 * 1024 * 1024;
const INLINE_BYTES: usize = 64 * 1024;
#[derive(Clone, Serialize, Deserialize)]
struct Blob {
    hash: String,
    bytes: usize,
}
#[derive(Clone, Serialize, Deserialize)]
struct Buffers {
    text: Blob,
    saved: Blob,
}
#[derive(Serialize)]
struct Checkpoint {
    #[serde(flatten)]
    workspace: Workspace,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    buffers: BTreeMap<u64, Buffers>,
}

// Deserialize the workspace directly: serde's flatten buffering does not
// support the numeric map keys used for document/view IDs.
fn parse(bytes: &[u8]) -> Result<Checkpoint> {
    #[derive(Deserialize)]
    struct Payloads {
        #[serde(default)]
        buffers: BTreeMap<u64, Buffers>,
    }
    Ok(Checkpoint {
        workspace: serde_json::from_slice(bytes)?,
        buffers: serde_json::from_slice::<Payloads>(bytes)?.buffers,
    })
}

/// What this process knows about the blobs in one buffers folder, so an
/// unchanged large buffer is neither hashed nor read back at every
/// checkpoint (they run about once a second while anything changes).
#[derive(Default)]
struct Known {
    /// The ropes of the last checkpoint and their blobs. A rope that is the
    /// same instance (`Rope::is_instance`) holds the same text: ropes copy on
    /// write, and these clones keep the shared roots alive.
    ropes: Vec<(Rope, Blob)>,
    /// Blobs this process wrote, or read back and checked, with the state
    /// of their files then. While that state is unchanged the content is
    /// too; any rewrite, replacement or resize means checking it again.
    verified: BTreeMap<String, Stamp>,
}
/// A blob file's identity and the times any write or replacement moves.
#[derive(PartialEq)]
struct Stamp {
    len: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}
fn stamp(path: &Path) -> Option<Stamp> {
    let meta = fs::symlink_metadata(path).ok().filter(|m| m.is_file())?;
    Some(Stamp {
        len: meta.len(),
        modified: meta.modified().ok(),
        #[cfg(unix)]
        identity: {
            use std::os::unix::fs::MetadataExt;
            (meta.dev(), meta.ino(), meta.ctime(), meta.ctime_nsec())
        },
    })
}
static KNOWN: Mutex<BTreeMap<PathBuf, Known>> = Mutex::new(BTreeMap::new());
fn known() -> std::sync::MutexGuard<'static, BTreeMap<PathBuf, Known>> {
    // Also used while unwinding (`forget` runs in `Drop`): never panic.
    KNOWN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
/// Release what is known about a store's blobs when it closes.
pub(crate) fn forget(store: &Path) {
    if let Some(dir) = store.parent() {
        known().remove(&dir.join("buffers"));
    }
}
#[cfg(test)]
thread_local! {
    /// (ropes hashed, blobs read back) on this thread.
    static WORK: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
    /// Holds this thread's next blob write until a message (or a timeout).
    static PAUSE: std::cell::RefCell<Option<std::sync::mpsc::Receiver<()>>> =
        const { std::cell::RefCell::new(None) };
}

fn hash_rope(rope: &Rope) -> String {
    #[cfg(test)]
    WORK.with(|w| w.set((w.get().0 + 1, w.get().1)));
    let mut hash = Sha256::new();
    for chunk in rope.chunks() {
        hash.update(chunk.as_bytes());
    }
    format!("{:x}", hash.finalize())
}
fn write_blob(
    dir: &Path,
    rope: &Rope,
    known: &mut Known,
    used: &mut Vec<(Rope, Blob)>,
) -> Result<Blob> {
    #[cfg(test)]
    PAUSE.with(|pause| {
        if let Some(resume) = pause.borrow_mut().take() {
            let _ = resume.recv_timeout(std::time::Duration::from_secs(10));
        }
    });
    anyhow::ensure!(
        rope.len_bytes() <= MAX_FILE_BYTES,
        "Recovery buffer exceeds 1 GiB"
    );
    let blob = match known.ropes.iter().find(|(r, _)| r.is_instance(rope)) {
        Some((_, blob)) => blob.clone(),
        None => Blob {
            hash: hash_rope(rope),
            bytes: rope.len_bytes(),
        },
    };
    used.push((rope.clone(), blob.clone()));
    let path = dir.join(&blob.hash);
    let current = stamp(&path);
    if current.is_some() && known.verified.get(&blob.hash) == current.as_ref() {
        return Ok(blob);
    }
    // Changed (or never checked) by this process: reusing a blob must verify
    // its contents, not just its name/size, so a successful retry repairs
    // corruption rather than acknowledging it.
    known.verified.remove(&blob.hash);
    if current.is_some() && read_blob(dir, &blob).is_ok() {
        if let Some(checked) = stamp(&path) {
            known.verified.insert(blob.hash.clone(), checked);
        }
        return Ok(blob);
    }
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    for chunk in rope.chunks() {
        file.write_all(chunk.as_bytes())?;
    }
    file.as_file().sync_all()?;
    file.persist(&path).map_err(|e| e.error)?;
    if let Some(written) = stamp(&path) {
        known.verified.insert(blob.hash.clone(), written);
    }
    Ok(blob)
}
fn read_blob(dir: &Path, blob: &Blob) -> Result<Rope> {
    #[cfg(test)]
    WORK.with(|w| w.set((w.get().0, w.get().1 + 1)));
    anyhow::ensure!(
        blob.bytes <= MAX_FILE_BYTES
            && blob.hash.len() == 64
            && blob
                .hash
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "Invalid recovery buffer reference"
    );
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(dir.join(&blob.hash))
        .context("Recovery buffer is missing")?;
    let meta = file.metadata()?;
    anyhow::ensure!(
        meta.is_file() && meta.len() == blob.bytes as u64,
        "Recovery buffer size does not match its checkpoint"
    );
    struct Hashed<R> {
        reader: R,
        hash: Sha256,
    }
    impl<R: Read> Read for Hashed<R> {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            let n = self.reader.read(bytes)?;
            self.hash.update(&bytes[..n]);
            Ok(n)
        }
    }
    let mut reader = Hashed {
        reader: file.take(blob.bytes as u64 + 1),
        hash: Sha256::new(),
    };
    let rope = Rope::from_reader(&mut reader)?;
    anyhow::ensure!(
        rope.len_bytes() == blob.bytes && format!("{:x}", reader.hash.finalize()) == blob.hash,
        "Recovery buffer failed its integrity check"
    );
    Ok(rope)
}
fn hydrate(path: &Path, document: &mut Document, buffers: &Buffers) -> Result<()> {
    anyhow::ensure!(
        document.recovery_external
            && !document.reference
            && document.recovery_ropes().0.len_bytes() == 0
            && document.recovery_ropes().1.len_bytes() == 0,
        "Invalid external recovery document"
    );
    let dir = path
        .parent()
        .context("Recovery directory missing")?
        .join("buffers");
    // A blob that fails its check is written again by the next checkpoint.
    let distrust = |blob: &Blob| {
        if let Some(known) = known().get_mut(&dir) {
            known.verified.remove(&blob.hash);
        }
    };
    let text = read_blob(&dir, &buffers.text).inspect_err(|_| distrust(&buffers.text))?;
    let saved = read_blob(&dir, &buffers.saved).inspect_err(|_| distrust(&buffers.saved))?;
    document.set_recovery_ropes(text, saved);
    document.recovery_external = false;
    Ok(())
}
pub(crate) fn decode(path: &Path, bytes: &[u8]) -> Result<Workspace> {
    let mut checkpoint = parse(bytes)?;
    anyhow::ensure!(
        checkpoint.buffers.is_empty() || checkpoint.workspace.version == 2,
        "External buffers require checkpoint schema 2"
    );
    for (id, buffers) in checkpoint.buffers {
        hydrate(
            path,
            checkpoint
                .workspace
                .documents
                .get_mut(&id)
                .context("Recovery buffer has no document")?,
            &buffers,
        )?;
    }
    for doc in checkpoint.workspace.documents.values_mut() {
        anyhow::ensure!(
            !doc.recovery_external,
            "Recovery document is missing its buffer references"
        );
        doc.restore_versions();
    }
    Ok(checkpoint.workspace)
}
pub(crate) fn salvage(path: &Path, bytes: &[u8], root: &Path) -> Option<Workspace> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let buffers = value.get("buffers").and_then(serde_json::Value::as_object);
    Workspace::salvage_with(bytes, root, |id, doc| {
        if let Some(raw) = buffers.and_then(|b| b.get(&id.to_string())) {
            let buffers: Buffers = serde_json::from_value(raw.clone())?;
            anyhow::ensure!(value["version"] == 2, "Invalid buffer schema");
            hydrate(path, doc, &buffers)?;
        }
        doc.restore_versions();
        Ok(())
    })
}

pub(crate) fn write(path: &Path, workspace: &Workspace) -> Result<()> {
    workspace
        .validate(&workspace.root)
        .context("Workspace is outside recovery limits; close excess tabs or save documents")?;
    let parent = path.parent().context("Recovery directory missing")?;
    // Say so plainly rather than failing every checkpoint with a bare error.
    anyhow::ensure!(
        parent.is_dir(),
        "Recovery folder {} was removed by another program",
        parent.display()
    );
    let mut checkpoint = Checkpoint {
        workspace: Workspace {
            version: 1,
            root: workspace.root.clone(),
            documents: workspace
                .documents
                .iter()
                .map(|(id, d)| (*id, d.checkpoint()))
                .collect(),
            views: workspace.views.clone(),
            terminals: workspace.terminals.clone(),
            layout: workspace.layout.clone(),
            focus: workspace.focus,
            browser: workspace.browser.clone(),
            ids: workspace.ids,
            breakpoints: workspace.breakpoints.clone(),
        },
        buffers: BTreeMap::new(),
    };
    let dir = parent.join("buffers");
    // Taken out of the shared map for the duration: holding the map's lock
    // across this store's hashing, writes and syncs would stall every other
    // store's checkpoints (as the lock is process-wide). After a failure it
    // is simply forgotten: the next checkpoint verifies again.
    let mut known = known().remove(&dir).unwrap_or_default();
    let mut used = Vec::new();
    for (id, document) in &mut checkpoint.workspace.documents {
        let (text, saved) = document.recovery_ropes();
        if text.len_bytes().saturating_add(saved.len_bytes()) <= INLINE_BYTES {
            continue;
        }
        fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
        }
        let buffers = Buffers {
            text: write_blob(&dir, text, &mut known, &mut used)?,
            saved: write_blob(&dir, saved, &mut known, &mut used)?,
        };
        checkpoint.buffers.insert(*id, buffers);
        document.set_recovery_ropes(Rope::new(), Rope::new());
        document.recovery_external = true;
        checkpoint.workspace.version = 2;
    }
    known.ropes = used;
    self::known().insert(dir.clone(), known);
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    struct Limited<'a> {
        writer: &'a mut File,
        remaining: u64,
    }
    impl Write for Limited<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() as u64 > self.remaining {
                return Err(std::io::Error::other(
                    "Recovery metadata exceeds 128 MiB; save documents to protect your work",
                ));
            }
            let n = self.writer.write(bytes)?;
            self.remaining -= n as u64;
            Ok(n)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.writer.flush()
        }
    }
    serde_json::to_writer(
        Limited {
            writer: file.as_file_mut(),
            remaining: METADATA_LIMIT,
        },
        &checkpoint,
    )?;
    file.flush()?;
    file.as_file().sync_all()?;
    // Buffer directory entries must be durable before publishing references.
    #[cfg(unix)]
    if dir.is_dir() {
        File::open(&dir)?.sync_all()?;
    }
    file.persist(path).map_err(|e| e.error)?;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    // GC only after commit. Preserve blobs used by retained previous/damaged checkpoints.
    // An unreadable retained checkpoint disables GC rather than risk deleting user work.
    let _ = collect_buffers(parent, &dir);
    Ok(())
}
fn collect_buffers(parent: &Path, dir: &Path) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    let mut used = BTreeSet::new();
    for entry in fs::read_dir(parent)? {
        let path = entry?.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let bytes = read_metadata(&path)?;
        let value: serde_json::Value = serde_json::from_slice(&bytes)?;
        if let Some(raw) = value.get("buffers") {
            let buffers: BTreeMap<u64, Buffers> = serde_json::from_value(raw.clone())?;
            for buffers in buffers.values() {
                used.insert(buffers.text.hash.clone());
                used.insert(buffers.saved.hash.clone());
            }
        }
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.len() == 64 && name.bytes().all(|b| b.is_ascii_hexdigit()) && !used.contains(&name)
        {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}
/// Whether a checkpoint may hold the only copy of unsaved work: a buffer
/// whose text differs from its saved text (compared without loading
/// external buffers), a file that vanished, or anything unreadable.
pub(crate) fn holds_unsaved_work(bytes: &[u8]) -> bool {
    use serde_json::Value;
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return true;
    };
    // A schema this release does not know may keep work elsewhere.
    if !matches!(value.get("version").and_then(Value::as_u64), Some(1 | 2)) {
        return true;
    }
    let Some(documents) = value.get("documents").and_then(Value::as_object) else {
        return true;
    };
    let buffers = value.get("buffers").and_then(Value::as_object);
    documents.iter().any(|(id, doc)| {
        if doc.get("reference") == Some(&Value::Bool(true)) {
            return false;
        }
        let same = match buffers.and_then(|b| b.get(id)) {
            Some(b) => {
                let hash = |which: &str| b.get(which).and_then(|blob| blob.get("hash"));
                hash("text").is_some() && hash("text") == hash("saved")
            }
            None => doc.get("text").is_some() && doc.get("text") == doc.get("saved"),
        };
        !same
            || doc.get("format") != doc.get("saved_format")
            || doc.get("unavailable") == Some(&Value::Bool(true))
    })
}
pub(crate) fn read_metadata(path: &Path) -> Result<Vec<u8>> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    anyhow::ensure!(
        file.metadata()?.is_file(),
        "Recovery checkpoint is not a regular file"
    );
    let mut bytes = Vec::new();
    file.take(METADATA_LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > METADATA_LIMIT {
        bail!("Recovery metadata exceeds 128 MiB");
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        layout::{Node, View},
        workspace::WorkspaceStore,
        EditorView,
    };
    fn workspace(root: &Path, text: String) -> Workspace {
        Workspace {
            version: 1,
            root: root.into(),
            documents: BTreeMap::from([
                (10, Document::from_text(text).unwrap()),
                (12, Document::from_text("small unsaved".into()).unwrap()),
            ]),
            views: BTreeMap::from([
                (
                    11,
                    EditorView {
                        document: 10,
                        cursor: 5,
                        ..Default::default()
                    },
                ),
                (
                    13,
                    EditorView {
                        document: 12,
                        ..Default::default()
                    },
                ),
            ]),
            terminals: BTreeMap::new(),
            layout: Node::Pane {
                id: 2,
                tabs: vec![View::Editor(11), View::Editor(13)],
                active: 0,
            },
            focus: 2,
            browser: root.into(),
            ids: 20,
            breakpoints: BTreeMap::new(),
        }
    }
    #[test]
    fn session_larger_than_old_checkpoint_limit_restores_all_buffers() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = WorkspaceStore::acquire_in(root.path(), state.path()).unwrap();
        let size = METADATA_LIMIT as usize + 1024;
        let w = workspace(root.path(), "x".repeat(size));
        write(&store.path, &w).unwrap();
        assert!(fs::metadata(&store.path).unwrap().len() < 16 * 1024);
        let restored = store.load(root.path()).unwrap().unwrap();
        assert_eq!(restored.documents[&10].len(), size);
        assert_eq!(restored.documents[&10].slice(size - 3, size), "xxx");
        assert_eq!(restored.documents[&12].text(), "small unsaved");
        assert_eq!(restored.views[&11].cursor, 5);
        assert!(restored.documents[&10].dirty());
    }
    #[test]
    fn failed_write_keeps_previous_checkpoint_and_corrupt_blob_salvages_other_documents() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = WorkspaceStore::acquire_in(root.path(), state.path()).unwrap();
        let old = workspace(root.path(), "previous".into());
        write(&store.path, &old).unwrap();
        let previous = fs::read(&store.path).unwrap();
        let mut invalid = workspace(root.path(), "invalid view".into());
        invalid.views.get_mut(&11).unwrap().cursor = 1000;
        assert!(write(&store.path, &invalid).is_err());
        assert_eq!(fs::read(&store.path).unwrap(), previous);
        let dir = store.path.parent().unwrap().join("buffers");
        fs::write(&dir, "blocked directory").unwrap();
        let w = workspace(root.path(), "large".repeat(INLINE_BYTES));
        assert!(write(&store.path, &w).is_err());
        assert_eq!(fs::read(&store.path).unwrap(), previous);
        fs::remove_file(&dir).unwrap();
        write(&store.path, &w).unwrap();
        let checkpoint: Checkpoint = parse(&fs::read(&store.path).unwrap()).unwrap();
        let blob = &checkpoint.buffers[&10].text;
        fs::write(dir.join(&blob.hash), "y".repeat(blob.bytes)).unwrap();
        assert!(decode(&store.path, &fs::read(&store.path).unwrap()).is_err());
        let (restored, kept) = store.load_tolerant(root.path()).unwrap().unwrap();
        assert!(kept.unwrap().exists());
        assert!(!restored.documents.contains_key(&10));
        assert_eq!(restored.documents[&12].text(), "small unsaved");
        restored.validate(root.path()).unwrap();
        write(&store.path, &w).unwrap();
        assert!(store
            .load(root.path())
            .unwrap()
            .unwrap()
            .documents
            .contains_key(&10));
    }
    #[test]
    fn blobs_preserve_saved_text_formats_baselines_and_legacy_checkpoints() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("session.json");
        let source = root.path().join("original.txt");
        fs::write(&source, "original\r\n".repeat(INLINE_BYTES)).unwrap();
        let mut doc = Document::open(&source).unwrap();
        doc.replace(0, 0, "unsaved\n", 0).unwrap();
        let mut w = workspace(root.path(), "legacy".into());
        // Old inline schema remains readable.
        fs::write(&path, serde_json::to_vec(&w).unwrap()).unwrap();
        assert_eq!(
            decode(&path, &fs::read(&path).unwrap()).unwrap().documents[&10].text(),
            "legacy"
        );
        w.documents.insert(10, doc.recovery_copy());
        write(&path, &w).unwrap();
        let mut loaded = decode(&path, &fs::read(&path).unwrap()).unwrap();
        let doc = loaded.documents.get_mut(&10).unwrap();
        doc.restore_versions();
        assert!(doc.dirty());
        assert!(doc.encoded().unwrap().starts_with(b"unsaved\r\n"));
        fs::write(&source, "external change").unwrap();
        assert!(doc.save(None).is_err());
        doc.discard_changes();
        assert_eq!(doc.slice(0, 9), "original\n");
        assert!(!doc.dirty());
    }
    #[test]
    fn gc_preserves_retained_snapshots_and_rejects_missing_or_unsafe_payloads() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("session.json");
        let first = workspace(root.path(), "a".repeat(INLINE_BYTES + 1));
        write(&path, &first).unwrap();
        let previous = root.path().join("session.previous.json");
        fs::copy(&path, &previous).unwrap();
        let bytes = fs::read(&previous).unwrap();
        let old = parse(&bytes).unwrap();
        write(&path, &workspace(root.path(), "b".repeat(INLINE_BYTES + 1))).unwrap();
        assert!(decode(&previous, &bytes).is_ok());
        fs::remove_file(&previous).unwrap();
        write(&path, &workspace(root.path(), "small".into())).unwrap();
        assert!(!root
            .path()
            .join("buffers")
            .join(&old.buffers[&10].text.hash)
            .exists());
        assert!(decode(&path, &bytes).is_err());
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let mut missing = value.clone();
        missing["buffers"].as_object_mut().unwrap().remove("10");
        assert!(decode(&path, &serde_json::to_vec(&missing).unwrap()).is_err());
        let salvaged = salvage(&path, &serde_json::to_vec(&missing).unwrap(), root.path()).unwrap();
        assert!(!salvaged.documents.contains_key(&10));
        assert!(salvaged.documents.contains_key(&12));
        value["buffers"]["10"]["text"]["hash"] = "../../original.txt".into();
        assert!(decode(&path, &serde_json::to_vec(&value).unwrap())
            .err()
            .unwrap()
            .to_string()
            .contains("Invalid recovery buffer reference"));
    }
    /// Only buffers without unsaved changes.
    fn clean(root: &Path) -> Workspace {
        let mut w = workspace(root, String::new());
        w.documents.insert(12, Document::scratch());
        w.views.get_mut(&11).unwrap().cursor = 0;
        w
    }
    fn work() -> (usize, usize) {
        WORK.with(|w| w.replace((0, 0)))
    }
    #[test]
    fn unchanged_large_buffers_are_neither_rehashed_nor_reread() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = WorkspaceStore::acquire_in(root.path(), state.path()).unwrap();
        let mut w = workspace(root.path(), "x".repeat(INLINE_BYTES * 2));
        work();
        write(&store.path, &w).unwrap();
        assert_eq!(work(), (2, 0), "New text and saved ropes are hashed once");
        // A checkpoint for a cursor move: same ropes, same blobs.
        write(&store.path, &w).unwrap();
        assert_eq!(work(), (0, 0));
        // Changed text is hashed and written.
        w.documents.insert(
            10,
            Document::from_text("y".repeat(INLINE_BYTES * 2)).unwrap(),
        );
        write(&store.path, &w).unwrap();
        assert_eq!(work(), (2, 0));
        let buffers = |path: &Path| parse(&fs::read(path).unwrap()).unwrap().buffers;
        let blob = store
            .path
            .parent()
            .unwrap()
            .join("buffers")
            .join(&buffers(&store.path)[&10].text.hash);
        // A blob removed behind the process's back is noticed and rewritten.
        fs::remove_file(&blob).unwrap();
        write(&store.path, &w).unwrap();
        assert_eq!(work(), (0, 0));
        assert!(blob.exists());
        assert_eq!(
            store.load(root.path()).unwrap().unwrap().documents[&10].text(),
            "y".repeat(INLINE_BYTES * 2)
        );
        // Same-size corruption changes the file's state: it is read back,
        // found wrong and repaired.
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&blob, "z".repeat(INLINE_BYTES * 2)).unwrap();
        work();
        write(&store.path, &w).unwrap();
        assert_eq!(work(), (0, 1));
        assert_eq!(
            fs::read(&blob).unwrap(),
            "y".repeat(INLINE_BYTES * 2).as_bytes()
        );
        // Blobs this process did not write are verified before reuse.
        forget(&store.path);
        work();
        write(&store.path, &w).unwrap();
        assert_eq!(work(), (2, 2));
    }
    #[test]
    fn a_slow_checkpoint_never_delays_another_stores_checkpoint() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let (resume, paused) = std::sync::mpsc::channel();
        let (started, running) = std::sync::mpsc::channel();
        let path = first.path().join("session.json");
        let large = workspace(first.path(), "x".repeat(INLINE_BYTES * 2));
        let slow = std::thread::spawn(move || {
            PAUSE.with(|pause| *pause.borrow_mut() = Some(paused));
            started.send(()).unwrap();
            write(&path, &large).unwrap();
        });
        running.recv().unwrap();
        // The first store's blob phase is now held for up to 10 seconds.
        std::thread::sleep(std::time::Duration::from_millis(50));
        let started = std::time::Instant::now();
        let path = second.path().join("session.json");
        write(
            &path,
            &workspace(second.path(), "y".repeat(INLINE_BYTES * 2)),
        )
        .unwrap();
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "Checkpoints of different stores must not wait for each other"
        );
        resume.send(()).unwrap();
        slow.join().unwrap();
    }
    #[test]
    fn unsaved_work_is_recognized_without_loading_buffers() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("session.json");
        write(&path, &clean(root.path())).unwrap();
        assert!(!holds_unsaved_work(&fs::read(&path).unwrap()));
        write(&path, &workspace(root.path(), "x".repeat(INLINE_BYTES * 2))).unwrap();
        assert!(holds_unsaved_work(&fs::read(&path).unwrap()));
        write(&path, &workspace(root.path(), "small".into())).unwrap();
        assert!(holds_unsaved_work(&fs::read(&path).unwrap()));
        assert!(holds_unsaved_work(b"{not json"));
        // A newer schema is kept, whatever it holds.
        write(&path, &clean(root.path())).unwrap();
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["version"] = 3.into();
        assert!(holds_unsaved_work(&serde_json::to_vec(&value).unwrap()));
    }
    #[test]
    fn unused_workspaces_expire_only_when_closed_and_saved() {
        let state = tempfile::tempdir().unwrap();
        let roots: Vec<_> = (0..4).map(|_| tempfile::tempdir().unwrap()).collect();
        let stores: Vec<_> = roots
            .iter()
            .map(|root| WorkspaceStore::acquire_in(root.path(), state.path()).unwrap())
            .collect();
        let dirs: Vec<PathBuf> = stores
            .iter()
            .map(|s| s.path.parent().unwrap().to_path_buf())
            .collect();
        // 0: saved; 1: unsaved; 2: saved but still open; 3: saved, recent.
        for (index, store) in stores.iter().enumerate() {
            let w = if index == 1 {
                workspace(roots[index].path(), "x".repeat(INLINE_BYTES * 2))
            } else {
                clean(roots[index].path())
            };
            write(&store.path, &w).unwrap();
        }
        let mut stores = stores.into_iter();
        let (first, second, open) = (stores.next(), stores.next(), stores.next().unwrap());
        drop((first, second, stores));
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(100 * 24 * 3600);
        for dir in &dirs[..3] {
            for name in ["lock", "session.json"] {
                let file = fs::OpenOptions::new()
                    .write(true)
                    .open(dir.join(name))
                    .unwrap();
                file.set_modified(old).unwrap();
            }
        }
        let other = tempfile::tempdir().unwrap();
        let _current = WorkspaceStore::acquire_in(other.path(), state.path()).unwrap();
        assert!(!dirs[0].exists(), "Saved, unused recovery state expires");
        assert!(
            dirs[1].join("buffers").is_dir(),
            "Unsaved work is never removed"
        );
        assert!(dirs[2].exists(), "An open workspace is never removed");
        assert!(dirs[3].exists(), "Recently used state stays");
        drop(open);
        // Reopening marks it used again.
        drop(WorkspaceStore::acquire_in(roots[2].path(), state.path()).unwrap());
        drop(WorkspaceStore::acquire_in(other.path(), state.path()));
        assert!(dirs[2].exists());
    }
}
