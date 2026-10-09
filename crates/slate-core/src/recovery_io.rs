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
    path::Path,
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

fn hash_rope(rope: &Rope) -> String {
    let mut hash = Sha256::new();
    for chunk in rope.chunks() {
        hash.update(chunk.as_bytes());
    }
    format!("{:x}", hash.finalize())
}
fn write_blob(dir: &Path, rope: &Rope) -> Result<Blob> {
    anyhow::ensure!(
        rope.len_bytes() <= MAX_FILE_BYTES,
        "Recovery buffer exceeds 1 GiB"
    );
    let blob = Blob {
        hash: hash_rope(rope),
        bytes: rope.len_bytes(),
    };
    let path = dir.join(&blob.hash);
    // Reusing a blob must verify its contents, not just its name/size: a
    // successful retry must repair corruption rather than acknowledge it.
    if path.exists() && read_blob(dir, &blob).is_ok() {
        return Ok(blob);
    }
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    for chunk in rope.chunks() {
        file.write_all(chunk.as_bytes())?;
    }
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(blob)
}
fn read_blob(dir: &Path, blob: &Blob) -> Result<Rope> {
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
    let text = read_blob(&dir, &buffers.text)?;
    let saved = read_blob(&dir, &buffers.saved)?;
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
            text: write_blob(&dir, text)?,
            saved: write_blob(&dir, saved)?,
        };
        checkpoint.buffers.insert(*id, buffers);
        document.set_recovery_ropes(Rope::new(), Rope::new());
        document.recovery_external = true;
        checkpoint.workspace.version = 2;
    }
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
}
