//! Workspace trust. Opening a folder never runs code from it: language
//! servers, tasks, formatters and plugins configured by the project, and Git
//! features that execute repository configuration (fsmonitor, hooks), wait
//! until the user trusts the folder. Trust covers the folder and everything
//! inside it.
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

/// Folders the user trusts, one absolute path per line.
pub fn trusted_folders() -> Vec<PathBuf> {
    fs::read_to_string(store())
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .collect()
}

/// Whether `root` is inside a trusted folder.
pub fn is_trusted(root: &Path) -> bool {
    let root = canonical(root);
    trusted_folders().iter().any(|t| root.starts_with(t))
}

/// Trust or stop trusting `root`.
pub fn set_trusted(root: &Path, trusted: bool) -> anyhow::Result<()> {
    let root = canonical(root);
    let mut folders = trusted_folders();
    folders.retain(|f| *f != root);
    if trusted {
        folders.push(root);
    }
    let path = store();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut text = String::from("# Folders whose tools Slate may run. One path per line.\n");
    for f in folders {
        text.push_str(&f.to_string_lossy());
        text.push('\n');
    }
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    std::io::Write::write_all(&mut file, text.as_bytes())?;
    file.persist(&path).map_err(|e| e.error)?;
    Ok(())
}

use crate::App;
use anyhow::Result;
impl App {
    pub fn trusted(&self) -> bool {
        self.trusted
    }
    /// Trust (or restrict) this session only, without remembering it.
    pub fn set_session_trust(&mut self, trusted: bool) {
        self.trusted = trusted;
        self.trust_changed();
    }
    /// Ask whether to trust the workspace, then run `retry` if the answer
    /// is yes.
    pub(crate) fn ask_trust(&mut self, reason: &str, retry: crate::Command) {
        self.pending_trust = Some(retry);
        self.prompt = Some(crate::actions::prompt(
            "trust",
            format!("{} · {reason}", self.root.display()),
        ));
    }
    pub(crate) fn answer_trust(&mut self, trust: bool) -> Result<()> {
        self.prompt = None;
        let retry = self.pending_trust.take();
        if !trust {
            self.status = "Restricted mode · project tools were not run".into();
            return Ok(());
        }
        self.set_workspace_trust(true)?;
        if let Some(command) = retry {
            self.execute(command)?;
        }
        Ok(())
    }
    /// Start or stop what depends on trust.
    pub(crate) fn trust_changed(&mut self) {
        self.refresh_git();
        self.reload_tools();
    }
    pub(crate) fn set_workspace_trust(&mut self, trusted: bool) -> Result<()> {
        set_trusted(&self.root, trusted)?;
        self.trusted = trusted;
        self.status = if trusted {
            "Workspace trusted · project tools may run".into()
        } else {
            "Restricted mode · project tools will not run".into()
        };
        self.trust_changed();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn trust_covers_nested_folders_and_can_be_revoked() {
        let _serial = crate::paths::TEST_ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
        let project = dir.path().join("project");
        std::fs::create_dir_all(project.join("src")).unwrap();
        assert!(!super::is_trusted(&project));
        super::set_trusted(&project, true).unwrap();
        assert!(super::is_trusted(&project.join("src")));
        assert!(!super::is_trusted(dir.path()));
        super::set_trusted(&project, false).unwrap();
        assert!(!super::is_trusted(&project));
    }
}
