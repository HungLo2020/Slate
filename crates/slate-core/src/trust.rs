//! Workspace trust. Opening a folder never runs code from it: language
//! servers, formatters, tasks, debugging and project tools configured by the
//! project, and Git (whose repository configuration can name programs to
//! run), wait until the user trusts the folder.
//!
//! `trusted-folders` in the config directory lists one absolute path per
//! line. A trusted folder covers everything inside it; a line starting with
//! `!` restricts a folder inside a trusted one. The most specific entry for
//! a path decides.
use std::{
    fs,
    path::{Path, PathBuf},
};

fn store() -> PathBuf {
    crate::paths::config_dir().join("trusted-folders")
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Entries as (folder, trusted).
fn entries() -> Vec<(PathBuf, bool)> {
    fs::read_to_string(store())
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| match l.strip_prefix('!') {
            Some(path) => (PathBuf::from(path.trim()), false),
            None => (PathBuf::from(l), true),
        })
        .filter(|(p, _)| p.is_absolute())
        .collect()
}

/// Folders the user trusts.
pub fn trusted_folders() -> Vec<PathBuf> {
    entries()
        .into_iter()
        .filter(|(_, trusted)| *trusted)
        .map(|(p, _)| p)
        .collect()
}

fn decide(path: &Path, entries: &[(PathBuf, bool)]) -> bool {
    entries
        .iter()
        .filter(|(folder, _)| path.starts_with(folder))
        .max_by_key(|(folder, _)| folder.components().count())
        .is_some_and(|(_, trusted)| *trusted)
}

/// Whether `path` is inside a trusted folder (and not a restricted one).
pub fn is_trusted(path: &Path) -> bool {
    decide(&canonical(path), &entries())
}

/// Trust or restrict `root`. Restricting a folder inside a trusted one
/// records an explicit restriction.
pub fn set_trusted(root: &Path, trusted: bool) -> anyhow::Result<()> {
    let root = canonical(root);
    let mut list = entries();
    list.retain(|(folder, _)| *folder != root);
    if trusted || decide(&root, &list) {
        list.push((root, trusted));
    }
    let mut text = String::from(
        "# Folders whose tools Slate may run, one per line; `!` restricts a folder inside one.\n",
    );
    for (folder, trusted) in list {
        if !trusted {
            text.push('!');
        }
        text.push_str(&folder.to_string_lossy());
        text.push('\n');
    }
    crate::fsio::write_private(&store(), text.as_bytes())
}

use crate::App;
use anyhow::Result;
impl App {
    pub fn trusted(&self) -> bool {
        self.trusted
    }
    /// Whether project code at `path` may run: inside the trusted workspace
    /// or inside another trusted folder.
    pub(crate) fn trusted_path(&self, path: &Path) -> bool {
        let path = canonical(path);
        if self.trusted && path.starts_with(canonical(&self.root)) {
            return true;
        }
        // Checked on every event-loop pass for open documents; the decision
        // only changes when trust does.
        let mut cache = self.trust_cache.lock().unwrap();
        *cache
            .entry(path.clone())
            .or_insert_with(|| is_trusted(&path))
    }
    /// Trust (or restrict) this session only, without remembering it.
    pub fn set_session_trust(&mut self, trusted: bool) {
        self.trusted = trusted;
        self.trust_changed();
    }
    /// Ask whether to trust the workspace, then run `retry` if the answer
    /// is yes.
    pub(crate) fn ask_trust(&mut self, reason: &str, retry: crate::Command) {
        let root = self.root.clone();
        self.ask_trust_for(&root, reason, retry);
    }
    /// Ask whether to trust `folder` (the workspace or, for example, a
    /// repository that contains it).
    pub(crate) fn ask_trust_for(&mut self, folder: &Path, reason: &str, retry: crate::Command) {
        self.pending_trust = Some((folder.to_path_buf(), retry));
        let scope = if self.file_mode && folder == self.root {
            " (for this session)"
        } else {
            ""
        };
        self.prompt = Some(crate::actions::prompt(
            "trust",
            format!("{}{scope} · {reason}", folder.display()),
        ));
    }
    pub(crate) fn answer_trust(&mut self, trust: bool) -> Result<()> {
        self.prompt = None;
        let pending = self.pending_trust.take();
        if !trust {
            self.status = "Restricted mode · project tools were not run".into();
            return Ok(());
        }
        let (folder, retry) = match pending {
            Some((folder, retry)) => (folder, Some(retry)),
            None => (self.root.clone(), None),
        };
        if folder == self.root {
            self.set_workspace_trust(true)?;
        } else {
            set_trusted(&folder, true)?;
            self.status = format!("Trusted {}", folder.display());
            self.trust_changed();
        }
        if let Some(command) = retry {
            self.execute(command)?;
        }
        Ok(())
    }
    /// Start or stop what depends on trust.
    pub(crate) fn trust_changed(&mut self) {
        self.trust_cache.lock().unwrap().clear();
        self.refresh_git();
        self.reload_tools();
    }
    /// Trust or restrict the workspace. A single file's folder is trusted
    /// for the session only, so editing a download does not trust the
    /// whole Downloads folder.
    pub(crate) fn set_workspace_trust(&mut self, trusted: bool) -> Result<()> {
        if !self.file_mode {
            set_trusted(&self.root, trusted)?;
        }
        self.trusted = trusted;
        self.status = match (trusted, self.file_mode) {
            (true, false) => "Workspace trusted · project tools may run".into(),
            (true, true) => format!(
                "Trusted {} for this session · project tools may run",
                self.root.display()
            ),
            (false, _) => "Restricted mode · project tools will not run".into(),
        };
        self.trust_changed();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn trust_covers_nested_folders_and_restrictions_override_parents() {
        let _serial = crate::paths::TEST_ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
        let project = dir.path().join("project");
        let inner = project.join("vendor");
        std::fs::create_dir_all(inner.join("src")).unwrap();
        assert!(!super::is_trusted(&project));
        super::set_trusted(&project, true).unwrap();
        assert!(super::is_trusted(&inner.join("src")));
        assert!(!super::is_trusted(dir.path()));
        // Restricting a folder inside a trusted one sticks.
        super::set_trusted(&inner, false).unwrap();
        assert!(!super::is_trusted(&inner));
        assert!(!super::is_trusted(&inner.join("src")));
        assert!(super::is_trusted(&project));
        super::set_trusted(&inner, true).unwrap();
        assert!(super::is_trusted(&inner));
        super::set_trusted(&project, false).unwrap();
        assert!(!super::is_trusted(&project));
        assert_eq!(super::trusted_folders(), [inner.canonicalize().unwrap()]);
    }
}
