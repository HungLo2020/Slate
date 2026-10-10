//! Shared filesystem operations; GUI trees and TUI navigation use one model.
use crate::{
    services::{Entry, IoJob},
    App, Command,
};
use anyhow::{bail, Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub(crate) enum FileOperation {
    Create(PathBuf, bool),
    Rename(PathBuf, PathBuf),
    Trash(PathBuf),
}

impl FileOperation {
    pub(crate) fn run(&self) -> Result<(), String> {
        (|| -> Result<()> {
            match self {
                Self::Create(path, true) => fs::create_dir(path)?,
                Self::Create(path, false) => {
                    fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(path)?;
                }
                Self::Rename(from, to) => {
                    if to.exists() {
                        bail!("The destination already exists");
                    }
                    rename_no_replace(from, to)?;
                }
                Self::Trash(path) => {
                    let output = crate::process::run(
                        std::process::Command::new("gio")
                            .arg("trash")
                            .arg("--")
                            .arg(path),
                        None,
                        std::time::Duration::from_secs(30),
                        "Desktop Trash",
                    )
                    .map_err(|e| anyhow::anyhow!("{e}; desktop Trash requires gio"))?;
                    if !output.status.success() {
                        bail!("{}", String::from_utf8_lossy(&output.stderr));
                    }
                }
            }
            Ok(())
        })()
        .map_err(|e| format!("{e:#}"))
    }
}

/// A path that may no longer exist, with its folder resolved.
fn resolved(path: &Path) -> PathBuf {
    match (path.parent().map(Path::canonicalize), path.file_name()) {
        (Some(Ok(parent)), Some(name)) => parent.join(name),
        _ => path.to_path_buf(),
    }
}
fn rename_no_replace(from: &Path, to: &Path) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        let from = std::ffi::CString::new(from.as_os_str().as_bytes())?;
        let to = std::ffi::CString::new(to.as_os_str().as_bytes())?;
        let result = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                from.as_ptr(),
                libc::AT_FDCWD,
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        if fs::symlink_metadata(to).is_ok() {
            bail!("The destination already exists");
        }
        fs::rename(from, to)?;
        Ok(())
    }
}

/// Resolve directory navigation against the workspace, including symlinks.
pub(crate) fn workspace_directory(path: &Path, workspace: &Path) -> Result<PathBuf> {
    let path = path.canonicalize()?;
    if !path.starts_with(workspace) {
        bail!("Folder is outside the workspace");
    }
    if !path.is_dir() {
        bail!("Not a directory");
    }
    Ok(path)
}

pub(crate) fn listing(
    root: &Path,
    workspace: &Path,
    expanded: &[PathBuf],
    tree: bool,
) -> std::io::Result<Vec<Entry>> {
    fn children(
        path: &Path,
        workspace: &Path,
        depth: usize,
        expanded: &[PathBuf],
        tree: bool,
        out: &mut Vec<Entry>,
    ) -> std::io::Result<()> {
        workspace_directory(path, workspace).map_err(std::io::Error::other)?;
        if depth > 16 || out.len() >= 100_000 {
            return Ok(());
        }
        let visible = unignored_children(path);
        let mut rows: Vec<_> = fs::read_dir(path)?
            .take(100_000 - out.len())
            .filter_map(Result::ok)
            .map(|e| {
                let directory = e.path().is_dir();
                let name = e.file_name();
                Entry {
                    ignored: name == ".git" || visible.as_ref().is_some_and(|v| !v.contains(&name)),
                    name: name.to_string_lossy().into_owned(),
                    path: e.path().to_string_lossy().into_owned(),
                    directory,
                    depth,
                    expanded: tree
                        && directory
                        && expanded.contains(&e.path())
                        && workspace_directory(&e.path(), workspace).is_ok(),
                }
            })
            .collect();
        rows.sort_by(|a, b| {
            b.directory
                .cmp(&a.directory)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        for row in rows {
            if out.len() >= 100_000 {
                break;
            }
            let descend = row.expanded;
            let path = PathBuf::from(&row.path);
            out.push(row);
            if descend {
                let _ = children(&path, workspace, depth + 1, expanded, tree, out);
            }
        }
        Ok(())
    }
    let mut rows = Vec::new();
    let canonical = workspace_directory(root, workspace).map_err(std::io::Error::other)?;
    if let Some(parent) = canonical.parent().filter(|p| p.starts_with(workspace)) {
        rows.push(Entry {
            name: "..".into(),
            path: parent.to_string_lossy().into_owned(),
            directory: true,
            depth: 0,
            expanded: false,
            ignored: false,
        });
    }
    children(root, workspace, 0, expanded, tree, &mut rows)?;
    Ok(rows)
}

/// The names in `folder` that ignore rules (`.gitignore`, `.ignore`, Git's
/// exclude files, as project search reads them) keep; hidden files count as
/// kept. `None` when the folder cannot be walked, so nothing is marked.
fn unignored_children(folder: &Path) -> Option<std::collections::HashSet<std::ffi::OsString>> {
    let mut kept = std::collections::HashSet::new();
    let walk = ignore::WalkBuilder::new(folder)
        .max_depth(Some(1))
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .parents(true)
        .follow_links(false)
        .require_git(false)
        .build();
    for entry in walk {
        let entry = entry.ok()?;
        if entry.depth() == 1 {
            kept.insert(entry.file_name().to_os_string());
        }
    }
    Some(kept)
}

impl App {
    pub(crate) fn file_action(&mut self, name: &str, argument: &str) -> Result<()> {
        let selected = || {
            self.files
                .get(self.selected)
                .filter(|e| e.name != "..")
                .map(|e| PathBuf::from(&e.path))
                .context("Select a file or folder")
        };
        match name {
            "new-file" | "new-folder" if argument.trim().is_empty() => {
                self.execute(Command::Prompt { kind: name.into() })?
            }
            "new-file" | "new-folder" => {
                self.services
                    .io
                    .send(IoJob::FileOperation(FileOperation::Create(
                        self.browser.join(argument),
                        name == "new-folder",
                    )))?;
            }
            "rename-file" => {
                if argument.trim().is_empty() {
                    let source = selected()?;
                    self.execute(Command::Prompt { kind: name.into() })?;
                    self.pending_file = Some(source.clone());
                    self.prompt.as_mut().unwrap().input =
                        source.file_name().unwrap().to_string_lossy().into_owned();
                } else {
                    let source = self.pending_file.take().map(Ok).unwrap_or_else(selected)?;
                    self.ensure_entry_idle(&source)?;
                    let target = source.parent().unwrap().join(argument);
                    self.services
                        .io
                        .send(IoJob::FileOperation(FileOperation::Rename(source, target)))?;
                }
            }
            "trash-file" => {
                let source = selected()?;
                self.ensure_entry_idle(&source)?;
                if self.documents.values().any(|doc| {
                    doc.path.as_ref().is_some_and(|p| p.starts_with(&source)) && doc.dirty()
                }) {
                    bail!("Save or close modified documents before moving this entry to Trash");
                }
                self.pending_file = Some(source.clone());
                self.prompt = Some(crate::search::Prompt {
                    kind: "trash-file".into(),
                    input: source.to_string_lossy().into_owned(),
                    replacement: String::new(),
                    field: 0,
                    case_sensitive: false,
                    whole_word: false,
                });
            }
            "toggle-folder" | "expand-folder" | "collapse-folder" => {
                if self.terminal_frontend {
                    let path = if name == "collapse-folder" {
                        if self.browser == self.root {
                            return Ok(());
                        }
                        self.browser
                            .parent()
                            .context("Already at the workspace root")?
                            .to_path_buf()
                    } else {
                        selected()?
                    };
                    self.browse(path)?;
                } else {
                    let path = if argument.is_empty() {
                        selected()?
                    } else {
                        PathBuf::from(argument)
                    };
                    let path = if path.is_absolute() {
                        path
                    } else {
                        self.browser.join(path)
                    };
                    workspace_directory(&path, &self.root)?;
                    if name == "collapse-folder"
                        || (name == "toggle-folder" && self.expanded_folders.contains(&path))
                    {
                        self.expanded_folders.remove(&path);
                    } else {
                        self.expanded_folders.insert(path);
                    }
                    self.refresh();
                }
            }
            // The TUI browses one folder at a time; collapsing everything
            // returns it to the workspace root.
            "collapse-all-folders" if self.terminal_frontend => self.browse(self.root.clone())?,
            "collapse-all-folders" => {
                self.expanded_folders.clear();
                self.refresh();
            }
            "terminal-here" => {
                let path = if argument.is_empty() {
                    selected()?
                } else {
                    PathBuf::from(argument)
                };
                let folder = if path.is_dir() {
                    path
                } else {
                    path.parent()
                        .context("Select a file or folder")?
                        .to_path_buf()
                };
                let folder = workspace_directory(&folder, &self.root)?;
                self.expand_workspace()?;
                let id = self.new_terminal_in(&folder)?;
                self.add_tab(crate::layout::View::Terminal(id));
            }
            _ => bail!("Unknown file action"),
        }
        Ok(())
    }
    pub(crate) fn confirm_trash(&mut self) -> Result<()> {
        let source = self.pending_file.take().context("No entry selected")?;
        self.ensure_entry_idle(&source)?;
        if self
            .documents
            .values()
            .any(|doc| doc.path.as_ref().is_some_and(|p| p.starts_with(&source)) && doc.dirty())
        {
            bail!("The entry has modified documents; save them first");
        }
        self.services
            .io
            .send(IoJob::FileOperation(FileOperation::Trash(source)))?;
        Ok(())
    }
    fn ensure_entry_idle(&self, source: &Path) -> Result<()> {
        if self.saves.save_operations.values().any(|doc| {
            doc.path
                .as_ref()
                .is_some_and(|path| path.starts_with(source))
        }) {
            bail!("Wait for saves to finish before renaming or moving this entry to Trash");
        }
        Ok(())
    }
    pub(crate) fn file_operation_done(
        &mut self,
        operation: FileOperation,
        result: Result<(), String>,
    ) {
        if let Err(error) = result {
            self.status = format!("File operation failed: {error}");
            return;
        }
        match operation {
            FileOperation::Create(path, false) => {
                let _ = self.execute(Command::Open { path });
            }
            FileOperation::Rename(from, to) => {
                // Document paths stay canonical: the target may have been
                // typed as `../name` or through a symlinked folder.
                let (from, to) = (resolved(&from), to.canonicalize().unwrap_or(to));
                // Joining an empty suffix would add a trailing separator.
                let moved = |suffix: &Path| {
                    if suffix.as_os_str().is_empty() {
                        to.clone()
                    } else {
                        to.join(suffix)
                    }
                };
                for doc in self.documents.values_mut() {
                    if let Some(suffix) = doc.path.as_ref().and_then(|p| p.strip_prefix(&from).ok())
                    {
                        doc.path = Some(moved(suffix));
                        doc.generation += 1;
                    }
                }
                self.expanded_folders = self
                    .expanded_folders
                    .iter()
                    .map(|path| {
                        path.strip_prefix(&from)
                            .map(moved)
                            .unwrap_or_else(|_| path.clone())
                    })
                    .collect();
                if let Ok(suffix) = self.browser.strip_prefix(&from) {
                    self.browser = moved(suffix);
                }
            }
            FileOperation::Trash(path) => {
                for doc in self.documents.values_mut() {
                    if doc.path.as_ref().is_some_and(|p| p.starts_with(&path)) {
                        doc.mark_unavailable();
                    }
                }
            }
            _ => {}
        }
        self.index_at = None;
        self.refresh_index();
        self.refresh();
        self.recovery.dirty = true;
        self.status = "File operation completed".into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_marks_ignored_entries_without_hiding_them() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("target/debug")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join(".gitignore"), "target/\n*.log\n").unwrap();
        fs::write(root.join("build.log"), "").unwrap();
        fs::write(root.join(".env"), "").unwrap();
        fs::write(root.join("src/main.rs"), "").unwrap();
        fs::write(root.join("src/trace.log"), "").unwrap();
        let rows = listing(&root, &root, &[root.join("src")], true).unwrap();
        let ignored = |name: &str| rows.iter().find(|e| e.name == name).unwrap().ignored;
        assert!(ignored(".git") && ignored("target") && ignored("build.log"));
        assert!(ignored("trace.log"), "nested rows use their parents' rules");
        // Hidden files are not ignored files.
        assert!(!ignored(".env") && !ignored(".gitignore") && !ignored("src"));
        assert!(!ignored("main.rs"));
    }

    #[cfg(unix)]
    #[test]
    fn tree_listing_does_not_follow_symlinks_outside_the_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("workspace");
        let inside = root.join("inside");
        let outside = dir.path().join("outside");
        fs::create_dir_all(&inside).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(inside.join("visible.txt"), "inside").unwrap();
        fs::write(outside.join("hidden.txt"), "outside").unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink(&inside, &link).unwrap();
        let rows = listing(&root, &root, std::slice::from_ref(&link), true).unwrap();
        assert!(rows.iter().any(|e| e.depth == 1 && e.name == "visible.txt"));
        assert!(!rows.iter().any(|e| e.name == ".."));

        // The link may have changed after expansion was queued.
        fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let rows = listing(&root, &root, std::slice::from_ref(&link), true).unwrap();
        assert!(rows.iter().any(|e| e.name == "link" && !e.expanded));
        assert!(!rows.iter().any(|e| e.name == "hidden.txt"));
        assert!(listing(&link, &root, &[], false).is_err());
        assert!(listing(&outside, &root, &[], true).is_err());
    }
    #[test]
    fn rename_never_replaces_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let target = dir.path().join("target");
        fs::write(&source, "source data").unwrap();
        fs::write(&target, "target data").unwrap();
        assert!(FileOperation::Rename(source.clone(), target.clone())
            .run()
            .is_err());
        assert_eq!(fs::read_to_string(source).unwrap(), "source data");
        assert_eq!(fs::read_to_string(target).unwrap(), "target data");
    }
    #[test]
    fn trash_refuses_modified_documents_and_rename_waits_for_saves() {
        let _serial = crate::paths::TEST_ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
        std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
        let path = dir.path().join("source.txt");
        fs::write(&path, "data").unwrap();
        let mut app = App::new(&path).unwrap();
        app.files = vec![Entry {
            name: "source.txt".into(),
            path: path.to_string_lossy().into_owned(),
            directory: false,
            depth: 0,
            expanded: false,
            ignored: false,
        }];
        let id = *app.documents.keys().next().unwrap();
        app.documents
            .get_mut(&id)
            .unwrap()
            .replace(0, 0, "edited", 0)
            .unwrap();
        assert!(app
            .file_action("trash-file", "")
            .unwrap_err()
            .to_string()
            .contains("modified"));
        app.saves
            .save_operations
            .insert(id, app.documents[&id].checkpoint());
        assert!(app
            .file_action("rename-file", "renamed.txt")
            .unwrap_err()
            .to_string()
            .contains("Wait for saves"));
        assert!(path.exists());
        assert!(!dir.path().join("renamed.txt").exists());
    }
    #[test]
    fn renaming_an_open_file_keeps_a_canonical_savable_path() {
        let _serial = crate::paths::TEST_ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
        std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
        let root = dir.path().canonicalize().unwrap();
        fs::create_dir(root.join("sub")).unwrap();
        std::os::unix::fs::symlink(root.join("sub"), root.join("link")).unwrap();
        let path = root.join("sub/source.txt");
        fs::write(&path, "data").unwrap();
        let mut app = App::new(&path).unwrap();
        let id = *app.documents.keys().next().unwrap();
        // Renamed out through `..`, then back in through a symlinked folder.
        for (from, to, expected) in [
            (
                path.clone(),
                root.join("sub/../moved.txt"),
                root.join("moved.txt"),
            ),
            (
                root.join("moved.txt"),
                root.join("link/back.txt"),
                root.join("sub/back.txt"),
            ),
        ] {
            FileOperation::Rename(from.clone(), to.clone())
                .run()
                .unwrap();
            app.file_operation_done(FileOperation::Rename(from, to), Ok(()));
            assert_eq!(app.documents[&id].path.as_deref(), Some(&*expected));
            assert_eq!(app.document_for(&expected), Some(id));
        }
        let doc = app.documents.get_mut(&id).unwrap();
        doc.replace(0, 0, "saved ", 0).unwrap();
        doc.save(None).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("sub/back.txt")).unwrap(),
            "saved data"
        );
    }
}
