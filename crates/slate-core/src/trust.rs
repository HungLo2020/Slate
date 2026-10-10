//! Workspace trust. Opening a folder never runs code from it: language
//! servers, formatters, tasks, debugging and project tools configured by the
//! project, and Git (whose repository configuration can name programs to
//! run), wait until the user trusts the folder.
//!
//! `trusted-folders` in the config directory lists one absolute path per
//! line. A trusted folder covers everything inside it; a line starting with
//! `!` restricts a folder inside a trusted one. The most specific entry for
//! a path decides. Paths are stored byte for byte; a folder whose name is
//! not UTF-8 or contains a line break can only be trusted for a session.
//!
//! Answering a prompt about a folder other than the workspace trusts it for
//! the session only, unless it is a repository's root. Broad folders (the
//! filesystem root, home, temporary and runtime folders, and folders that
//! contain them) are never remembered.
use std::path::{Path, PathBuf};

fn store() -> PathBuf {
    crate::paths::config_dir().join("trusted-folders")
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(unix)]
fn from_bytes(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
}
#[cfg(not(unix))]
fn from_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}
#[cfg(unix)]
fn to_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}
#[cfg(not(unix))]
fn to_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().into_owned().into_bytes()
}

/// Lines as stored, (folder, trusted). Leading blanks are ignored (paths are
/// absolute); trailing ones belong to the name, except a CRLF line ending.
fn parse(bytes: &[u8]) -> Vec<(PathBuf, bool)> {
    bytes
        .split(|b| *b == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line).trim_ascii_start())
        .filter(|l| !l.is_empty() && !l.starts_with(b"#"))
        .map(|l| match l.strip_prefix(b"!") {
            Some(path) => (from_bytes(path.trim_ascii_start()), false),
            None => (from_bytes(l), true),
        })
        .filter(|(p, _)| p.is_absolute())
        .collect()
}
fn stored() -> std::io::Result<Vec<(PathBuf, bool)>> {
    match crate::fsio::read_regular(&store(), crate::config::MAX_CONFIG_BYTES) {
        Ok(bytes) => Ok(parse(&bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}
/// Entries that decide trust. Restrictions fail closed: older releases
/// trimmed lines, so a restriction also covers its name without trailing
/// blanks.
fn effective(mut list: Vec<(PathBuf, bool)>) -> Vec<(PathBuf, bool)> {
    let trimmed: Vec<_> = list
        .iter()
        .filter(|(_, trusted)| !trusted)
        .filter_map(|(folder, _)| {
            let bytes = to_bytes(folder);
            let short = bytes.trim_ascii_end();
            (short.len() != bytes.len()).then(|| (from_bytes(short), false))
        })
        .collect();
    // Placed first: an exact entry for the same folder still decides.
    list.splice(0..0, trimmed);
    list
}
/// Entries as (folder, trusted); unreadable means nothing is trusted.
fn entries() -> Vec<(PathBuf, bool)> {
    effective(stored().unwrap_or_default())
}

/// Folders the user trusts.
pub fn trusted_folders() -> Vec<PathBuf> {
    stored()
        .unwrap_or_default()
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

/// Whether `folder` can be written as one line of the trust file.
fn storable(folder: &Path) -> bool {
    folder.to_str().is_some_and(|s| !s.contains(['\n', '\r']))
}

/// Folders that hold unrelated projects and files: trusting one would trust
/// everything below it. Also every folder that contains one of them.
pub(crate) fn broad(folder: &Path) -> bool {
    let folder = canonical(folder);
    let mut wide = vec![
        PathBuf::from("/"),
        PathBuf::from("/tmp"),
        PathBuf::from("/var/tmp"),
        std::env::temp_dir(),
    ];
    wide.extend(std::env::var_os("HOME").map(PathBuf::from));
    wide.extend(std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from));
    wide.iter()
        .filter(|w| w.is_absolute())
        .any(|w| canonical(w).starts_with(&folder))
}

/// Trust or restrict `root`. Restricting a folder inside a trusted one
/// records an explicit restriction.
pub fn set_trusted(root: &Path, trusted: bool) -> anyhow::Result<()> {
    let root = canonical(root);
    anyhow::ensure!(
        storable(&root),
        "Cannot remember trust for {}: its name is not plain text on one line",
        root.display()
    );
    anyhow::ensure!(
        !trusted || !broad(&root),
        "{} holds unrelated files; trust a project folder inside it instead",
        root.display()
    );
    // Other instances update the same file: re-read it under the lock.
    crate::fsio::with_lock(&store(), || {
        let mut list = stored()?;
        list.retain(|(folder, _)| *folder != root);
        if trusted || decide(&root, &effective(list.clone())) {
            list.push((root.clone(), trusted));
        }
        let mut text = Vec::from(
            &b"# Folders whose tools Slate may run, one per line; `!` restricts a folder inside one.\n"[..],
        );
        for (folder, trusted) in list {
            if !trusted {
                text.push(b'!');
            }
            text.extend(to_bytes(&folder));
            text.push(b'\n');
        }
        crate::fsio::write_private(&store(), &text)
    })
}

use crate::App;
use anyhow::Result;
impl App {
    pub fn trusted(&self) -> bool {
        self.trusted
    }
    /// Whether project code at `path` may run: inside the trusted workspace
    /// or inside another folder trusted persistently or for this session.
    pub(crate) fn trusted_path(&self, path: &Path) -> bool {
        // Checked on every event-loop pass for open documents; the decision
        // only changes when trust does. Looked up by the path as asked, so
        // a repeated question touches no file system.
        if let Some(trusted) = self.trust_cache.lock().unwrap().get(path) {
            return *trusted;
        }
        let asked = path;
        let path = canonical(path);
        let trusted = {
            let root = canonical(&self.root);
            // An unreadable trust file may hold restrictions: fail closed.
            let (list, readable) = self.decisions();
            if path.starts_with(&root) {
                // Session trust overrides the root entry, but never a more
                // specific persisted decision about a child folder.
                let fallback = self.trusted && (readable || path == root);
                list.iter()
                    .filter(|(folder, _)| {
                        folder != &root && folder.starts_with(&root) && path.starts_with(folder)
                    })
                    .max_by_key(|(folder, _)| folder.components().count())
                    .map_or(fallback, |(_, trusted)| *trusted)
            } else {
                decide(&path, &list)
            }
        };
        self.trust_cache
            .lock()
            .unwrap()
            .insert(asked.to_path_buf(), trusted);
        trusted
    }
    /// Remembered entries, then this session's answers (after, so ahead of
    /// a remembered decision about the same folder); and whether the trust
    /// file could be read.
    fn decisions(&self) -> (Vec<(PathBuf, bool)>, bool) {
        let (mut list, readable) = match stored() {
            Ok(list) => (effective(list), true),
            Err(_) => (Vec::new(), false),
        };
        list.extend(self.session_trust.iter().map(|f| (f.clone(), true)));
        (list, readable)
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
    /// Whether answering yes for `folder` is remembered. A single file's
    /// folder, broad folders and names the trust file cannot hold are
    /// trusted for the session; another folder is remembered only when it is
    /// a repository's root, never a file's arbitrary parent folder.
    fn remembers_trust(&self, folder: &Path) -> bool {
        let workspace = folder == self.root;
        !(workspace && self.file_mode)
            && (workspace || folder.join(".git").exists())
            && !broad(folder)
            && storable(&canonical(folder))
    }
    /// Ask whether to trust `folder` (the workspace or, for example, a
    /// repository that contains it). The answer covers everything inside.
    pub(crate) fn ask_trust_for(&mut self, folder: &Path, reason: &str, retry: crate::Command) {
        self.pending_trust = Some((folder.to_path_buf(), retry));
        let scope = if broad(folder) {
            " (for this session) · it holds unrelated files: trust covers every file and project inside it"
        } else if self.remembers_trust(folder) {
            " · trust covers everything inside it"
        } else {
            " (for this session) · trust covers everything inside it"
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
            if self.remembers_trust(&folder) {
                set_trusted(&folder, true)?;
                self.status = format!("Trusted {}", folder.display());
            } else {
                self.session_trust.push(canonical(&folder));
                self.status = format!("Trusted {} for this session", folder.display());
            }
            // Trusting a folder that contains the workspace trusts it too
            // (unless a more specific decision restricts it): ask once.
            let root = canonical(&self.root);
            if !self.trusted && root.starts_with(canonical(&folder)) {
                self.trusted = decide(&root, &self.decisions().0);
            }
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
        self.trust_generation += 1;
        self.refresh_git();
        self.reload_tools();
    }
    /// Trust or restrict the workspace. A single file's folder (and a broad
    /// folder) is trusted for the session only, so editing a download does
    /// not trust the whole Downloads folder.
    pub(crate) fn set_workspace_trust(&mut self, trusted: bool) -> Result<()> {
        let root = self.root.clone();
        let remembered = !trusted || self.remembers_trust(&root);
        // A restriction applies now even if it cannot be remembered.
        self.trusted &= trusted;
        if remembered && !self.file_mode {
            set_trusted(&root, trusted)?;
        }
        self.trusted = trusted;
        self.status = match (trusted, remembered) {
            (true, true) => "Workspace trusted · project tools may run".into(),
            (true, false) => format!(
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
    use std::path::{Path, PathBuf};

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
        std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
        let mut app = crate::App::new(&project).unwrap();
        assert!(app.trusted());
        assert!(app.trusted_path(&project));
        assert!(!app.trusted_path(&inner.join("src")));
        app.set_session_trust(true);
        assert!(
            !app.trusted_path(&inner.join("src")),
            "Session trust must respect child restrictions"
        );
        app.debug_start("vendor/program").unwrap();
        assert_eq!(app.pending_trust.as_ref().unwrap().0, inner);
        assert!(!app.debugging());
        super::set_trusted(&inner, true).unwrap();
        assert!(super::is_trusted(&inner));
        super::set_trusted(&project, false).unwrap();
        assert!(!super::is_trusted(&project));
        assert_eq!(super::trusted_folders(), [inner.canonicalize().unwrap()]);
    }
    #[test]
    fn answers_for_other_folders_are_scoped_and_broad_folders_never_remembered() {
        let _serial = crate::paths::TEST_ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
        std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
        let home = dir.path().join("home");
        let project = home.join("project");
        let downloads = home.join("Downloads");
        let repository = home.join("repository");
        for folder in [&project, &downloads, &repository.join(".git")] {
            std::fs::create_dir_all(folder).unwrap();
        }
        let saved_home = std::env::var_os("HOME");
        std::env::set_var("HOME", &home);
        let mut app = crate::App::new(&project).unwrap();
        let ask = |app: &mut crate::App, folder: &Path| {
            app.ask_trust_for(folder, "Formatters run", crate::Command::Refresh);
            app.prompt.as_ref().unwrap().input.clone()
        };
        // A file's folder is trusted for this session only.
        assert!(ask(&mut app, &downloads).contains("for this session"));
        app.answer_trust(true).unwrap();
        assert!(app.trusted_path(&downloads.join("file.py")));
        assert!(!super::is_trusted(&downloads));
        assert!(!app.trusted_path(&home), "Session trust stays scoped");
        // Home is broad: the prompt says so and nothing is written.
        assert!(ask(&mut app, &home).contains("unrelated files"));
        app.answer_trust(true).unwrap();
        assert!(app.trusted_path(&home.join(".bashrc")));
        assert!(!super::is_trusted(&home));
        assert!(super::set_trusted(&home, true).is_err());
        assert!(
            super::set_trusted(dir.path(), true).is_err(),
            "Ancestors too"
        );
        assert!(super::set_trusted(Path::new("/"), true).is_err());
        assert!(super::set_trusted(&std::env::temp_dir(), true).is_err());
        // A repository's root is a project: remembered.
        assert!(!ask(&mut app, &repository).contains("for this session"));
        app.answer_trust(true).unwrap();
        assert!(super::is_trusted(&repository));
        match saved_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn stored_paths_are_exact_and_restrictions_fail_closed() {
        let _serial = crate::paths::TEST_ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
        let root = dir.path().canonicalize().unwrap();
        // A line break cannot inject another entry.
        let injected = root.join("x\n/");
        std::fs::create_dir_all(&injected).unwrap();
        assert!(super::set_trusted(&injected, true).is_err());
        assert!(!super::is_trusted(Path::new("/etc")));
        // Trailing blanks belong to the folder's name.
        let spaced = root.join("project ");
        std::fs::create_dir_all(&spaced).unwrap();
        std::fs::create_dir_all(root.join("project")).unwrap();
        super::set_trusted(&spaced, true).unwrap();
        assert!(super::is_trusted(&spaced));
        assert!(!super::is_trusted(&root.join("project")));
        let store = super::store();
        // Hand-edited: CRLF endings, a restriction with a trailing blank, and
        // (on Unix) a restriction whose name is not UTF-8.
        let mut text = format!("{}\r\n!{}/inner \r\n", root.display(), root.display()).into_bytes();
        text.extend(format!("!{}/caf", root.display()).as_bytes());
        text.extend(b"\xe9\n");
        std::fs::write(&store, &text).unwrap();
        assert!(super::is_trusted(&root.join("other")));
        assert!(!super::is_trusted(&root.join("inner")));
        assert!(!super::is_trusted(&root.join("inner ")));
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let name = std::ffi::OsStr::from_bytes(b"caf\xe9");
            assert!(!super::is_trusted(&root.join(name).join("src")));
            assert!(super::set_trusted(&root.join(name), true).is_err());
            // Rewriting keeps the restriction byte for byte.
            super::set_trusted(&root.join("other"), true).unwrap();
            assert!(!super::is_trusted(&root.join(name)));
        }
    }

    #[test]
    fn concurrent_updates_keep_every_entry() {
        let _serial = crate::paths::TEST_ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
        let folders: Vec<PathBuf> = (0..8)
            .map(|i| {
                let folder = dir.path().join(format!("project{i}"));
                std::fs::create_dir_all(&folder).unwrap();
                folder.canonicalize().unwrap()
            })
            .collect();
        std::thread::scope(|scope| {
            for folder in &folders {
                scope.spawn(move || {
                    for _ in 0..5 {
                        super::set_trusted(folder, true).unwrap();
                    }
                });
            }
        });
        let mut trusted = super::trusted_folders();
        trusted.sort();
        assert_eq!(trusted, folders);
    }

    #[test]
    fn trusting_a_containing_folder_trusts_the_workspace_once() {
        let _serial = crate::paths::TEST_ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
        std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
        for (outer, remembered) in [("repository", true), ("plain", false)] {
            let outer = dir.path().join(outer);
            let project = outer.join("project");
            std::fs::create_dir_all(&project).unwrap();
            if remembered {
                std::fs::create_dir_all(outer.join(".git")).unwrap();
            }
            let mut app = crate::App::new(&project).unwrap();
            assert!(!app.trusted());
            app.ask_trust_for(&outer, "Git", crate::Command::Refresh);
            app.answer_trust(true).unwrap();
            assert!(app.trusted(), "No second prompt for the workspace");
            assert_eq!(super::is_trusted(&project), remembered);
        }
    }

    #[test]
    fn an_unreadable_trust_file_fails_closed_inside_the_workspace() {
        let _serial = crate::paths::TEST_ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
        std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
        let project = dir.path().join("project");
        std::fs::create_dir_all(project.join("vendor")).unwrap();
        let mut app = crate::App::new(&project).unwrap();
        app.set_session_trust(true);
        // No trust file: nothing is restricted.
        assert!(app.trusted_path(&project.join("vendor")));
        // Too large to read: restrictions it may hold still apply.
        std::fs::create_dir_all(super::store().parent().unwrap()).unwrap();
        std::fs::write(
            super::store(),
            vec![b'#'; crate::config::MAX_CONFIG_BYTES as usize + 1],
        )
        .unwrap();
        app.set_session_trust(true);
        assert!(app.trusted_path(&project));
        assert!(!app.trusted_path(&project.join("vendor")));
    }
}
