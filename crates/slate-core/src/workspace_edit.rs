//! Validate ordered LSP file operations before changing disk or editor state.
use crate::{
    document::Document,
    layout::View,
    lsp::{checked_text_edits, path_of},
    App, EditorView,
};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
const LIMIT: u64 = 16 * 1024 * 1024;
fn fingerprint(path: &Path) -> Result<crate::fsio::Baseline> {
    Ok(crate::fsio::Baseline::of(&std::fs::read(path)?))
}
struct Buffer {
    id: Option<u64>,
    doc: Document,
    groups: Vec<Vec<(usize, usize, String)>>,
}
enum Operation {
    Create(PathBuf),
    Rename(PathBuf, PathBuf),
    Delete(PathBuf),
}
enum Undo {
    Remove(PathBuf),
    Rename(PathBuf, PathBuf),
}
fn inside(root: &Path, value: &Value) -> Result<PathBuf> {
    let path = value
        .as_str()
        .and_then(path_of)
        .context("Invalid file URI")?;
    let parent = path
        .parent()
        .context("File has no parent")?
        .canonicalize()?;
    let real = parent.join(path.file_name().context("File has no name")?);
    anyhow::ensure!(
        real.starts_with(root),
        "File operation escapes the workspace"
    );
    if let Ok(meta) = std::fs::symlink_metadata(&real) {
        anyhow::ensure!(
            meta.is_file() && !meta.file_type().is_symlink(),
            "Resource edits require regular files; symlinks and directories are excluded"
        );
    }
    Ok(real)
}
impl App {
    pub(crate) fn apply_resource_edit(
        &mut self,
        edit: &Value,
        expected: Option<&BTreeMap<u64, u64>>,
    ) -> Result<usize> {
        let changes = edit["documentChanges"]
            .as_array()
            .context("Missing ordered edits")?;
        anyhow::ensure!(
            changes.len() <= 128,
            "Workspace edit exceeds 128 operations"
        );
        let root = self.root.canonicalize()?;
        let mut buffers: BTreeMap<PathBuf, Option<Buffer>> = BTreeMap::new();
        let mut originals = BTreeMap::new();
        let mut bytes = 0;
        let mut operations = Vec::new();
        for change in changes {
            let kind = change["kind"].as_str();
            let path = inside(
                &root,
                if kind == Some("rename") {
                    &change["oldUri"]
                } else if kind.is_some() {
                    &change["uri"]
                } else {
                    &change["textDocument"]["uri"]
                },
            )?;
            let mut paths = vec![path.clone()];
            if kind == Some("rename") {
                paths.push(inside(&root, &change["newUri"])?);
            }
            for path in &paths {
                if buffers.contains_key(path) {
                    continue;
                }
                if path.exists() {
                    bytes += std::fs::metadata(path)?.len();
                    anyhow::ensure!(
                        bytes <= LIMIT,
                        "Resource edit exceeds the 16 MiB transaction limit"
                    );
                }
                let disk = if path.exists() {
                    Some(fingerprint(path)?)
                } else {
                    None
                };
                originals.insert(path.clone(), disk);
                let id = self.document_for(path);
                let doc = if let Some(id) = id {
                    let d = &self.documents[&id];
                    anyhow::ensure!(
                        !d.read_only && !self.saves.pending_save.contains(&id),
                        "File is read-only or has a pending save"
                    );
                    anyhow::ensure!(
                        !expected
                            .is_some_and(|v| v.get(&id).is_some_and(|c| *c != d.content_version())),
                        "Document changed since the edit was requested"
                    );
                    Some(d.checkpoint())
                } else if path.exists() {
                    Some(Document::open(path)?)
                } else {
                    None
                };
                buffers.insert(
                    path.clone(),
                    doc.map(|doc| Buffer {
                        id,
                        doc,
                        groups: Vec::new(),
                    }),
                );
            }
            match kind {
                Some("create") => {
                    if buffers[&path].is_some() {
                        if change["options"]["ignoreIfExists"] == true
                            && change["options"]["overwrite"] != true
                        {
                            continue;
                        }
                        anyhow::ensure!(
                            change["options"]["overwrite"] == true,
                            "Create destination already exists"
                        );
                        let old = buffers[&path].as_ref().unwrap();
                        anyhow::ensure!(!old.doc.dirty(), "Create would overwrite unsaved work");
                        operations.push(Operation::Delete(path.clone()));
                    }
                    let old = buffers.get_mut(&path).unwrap().take();
                    let mut buffer = old.unwrap_or_else(|| Buffer {
                        id: None,
                        doc: Document::scratch(),
                        groups: Vec::new(),
                    });
                    if !buffer.doc.is_empty() {
                        let reset = vec![(0, buffer.doc.len(), String::new())];
                        buffer.doc.replace_many(&reset, 0)?;
                        buffer.groups.push(reset);
                    }
                    buffer.doc.path = Some(path.clone());
                    buffer.doc.disk = Some(crate::fsio::Baseline::of(&[]));
                    buffer.doc.stamp = None;
                    buffers.insert(path.clone(), Some(buffer));
                    operations.push(Operation::Create(path));
                }
                Some("rename") => {
                    let dest = paths[1].clone();
                    if path == dest {
                        continue;
                    }
                    anyhow::ensure!(buffers[&path].is_some(), "Rename source does not exist");
                    if let Some(target) = &buffers[&dest] {
                        if change["options"]["ignoreIfExists"] == true
                            && change["options"]["overwrite"] != true
                        {
                            continue;
                        }
                        anyhow::ensure!(
                            change["options"]["overwrite"] == true
                                && !target.doc.dirty()
                                && target.id.is_none(),
                            "Rename destination exists or is open; close it first"
                        );
                        operations.push(Operation::Delete(dest.clone()));
                    }
                    let mut source = buffers.get_mut(&path).unwrap().take().unwrap();
                    source.doc.path = Some(dest.clone());
                    buffers.insert(dest.clone(), Some(source));
                    operations.push(Operation::Rename(path, dest));
                }
                Some("delete") => {
                    let Some(source) = &buffers[&path] else {
                        if change["options"]["ignoreIfNotExists"] == true {
                            continue;
                        }
                        bail!("Delete source does not exist");
                    };
                    anyhow::ensure!(!source.doc.dirty(), "Delete would discard unsaved work");
                    buffers.insert(path.clone(), None);
                    operations.push(Operation::Delete(path));
                }
                Some(other) => bail!("Unknown resource operation: {other}"),
                None => {
                    let buffer = buffers
                        .get_mut(&path)
                        .unwrap()
                        .as_mut()
                        .context("Text edit names a deleted or missing file")?;
                    if let (Some(id), Some(version)) =
                        (buffer.id, change["textDocument"]["version"].as_i64())
                    {
                        anyhow::ensure!(
                            self.lsp_version(id).is_none_or(|v| v == version),
                            "Stale language-server document version"
                        );
                    }
                    let edits = checked_text_edits(&buffer.doc, &change["edits"])?;
                    buffer.doc.replace_many(&edits, 0)?;
                    buffer.groups.push(edits);
                }
            }
        }
        // Catch modifications that happened during validation, including destination creation.
        for (path, baseline) in &originals {
            let now = if path.exists() {
                Some(fingerprint(path)?)
            } else {
                None
            };
            anyhow::ensure!(
                now == *baseline,
                "File changed during validation; nothing was changed"
            );
        }
        let stash = tempfile::Builder::new()
            .prefix(".slate-edit-")
            .tempdir_in(&root)?;
        let mut undo = Vec::new();
        let applied = (|| -> Result<()> {
            for (index, op) in operations.iter().enumerate() {
                match op {
                    Operation::Create(path) => {
                        std::fs::OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .open(path)?;
                        undo.push(Undo::Remove(path.clone()));
                    }
                    Operation::Rename(from, to) => {
                        anyhow::ensure!(!to.exists(), "Rename destination appeared during commit");
                        std::fs::rename(from, to)?;
                        undo.push(Undo::Rename(to.clone(), from.clone()));
                    }
                    Operation::Delete(path) => {
                        let backup = stash.path().join(index.to_string());
                        std::fs::rename(path, &backup)?;
                        undo.push(Undo::Rename(backup, path.clone()));
                    }
                }
            }
            Ok(())
        })();
        if let Err(error) = applied {
            let mut failures = Vec::new();
            for undo in undo.into_iter().rev() {
                let result = match undo {
                    Undo::Remove(path) => std::fs::remove_file(path),
                    Undo::Rename(from, to) => std::fs::rename(from, to),
                };
                if let Err(e) = result {
                    failures.push(e.to_string());
                }
            }
            if !failures.is_empty() {
                let preserved = stash.keep();
                bail!(
                    "{error:#}; rollback failed: {}. Originals retained in {}",
                    failures.join("; "),
                    preserved.display()
                );
            }
            return Err(error);
        }
        let ids: std::collections::BTreeSet<_> =
            buffers.values().flatten().filter_map(|b| b.id).collect();
        for path in originals.keys() {
            if let Some(id) = self.document_for(path) {
                if !ids.contains(&id) {
                    self.documents.get_mut(&id).unwrap().mark_unavailable();
                }
            }
        }
        let mut count = 0;
        for (path, buffer) in buffers {
            let Some(buffer) = buffer else {
                continue;
            };
            if let Some(id) = buffer.id {
                for edits in buffer.groups {
                    self.replace_in_document(id, &edits)?;
                }
                let d = self.documents.get_mut(&id).unwrap();
                let created = operations
                    .iter()
                    .any(|op| matches!(op,Operation::Create(p) if *p == path));
                d.path = Some(path);
                if created {
                    d.disk = Some(crate::fsio::Baseline::of(&[]));
                    d.stamp = None;
                }
            } else if !buffer.groups.is_empty()
                || operations
                    .iter()
                    .any(|op| matches!(op,Operation::Create(p) if *p == path))
            {
                let doc = self.id();
                self.documents.insert(doc, buffer.doc);
                let view = self.id();
                self.views.insert(
                    view,
                    EditorView {
                        document: doc,
                        ..Default::default()
                    },
                );
                let preferred = self.pane_showing_editor();
                self.place_view_background(View::Editor(view), preferred);
            }
            count += 1;
        }
        self.refresh();
        self.recovery.dirty = true;
        Ok(count)
    }
}
