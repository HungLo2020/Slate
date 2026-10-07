use std::path::{Path, PathBuf};

fn base(variable: &str, fallback: &str) -> PathBuf {
    let value = std::env::var_os(variable)
        .map(PathBuf::from)
        .filter(|path| !foreign_to_root(path));
    resolve(value, home(), fallback)
}

/// The home directory for configuration and state. Under `sudo` without
/// `-H`, `HOME` still names the invoking user's directory; writing root-owned
/// files there would break that user's later sessions, so root uses its own.
fn home() -> PathBuf {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    if foreign_to_root(&home) {
        if let Some(root_home) = root_home() {
            return root_home;
        }
    }
    home
}

/// Running as root, inside a directory tree owned by another user.
#[cfg(unix)]
fn foreign_to_root(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    if unsafe { libc::geteuid() } != 0 {
        return false;
    }
    // The nearest existing ancestor decides who owns new files beneath it.
    path.ancestors()
        .find_map(|p| std::fs::metadata(p).ok())
        .is_some_and(|meta| meta.uid() != 0)
}
#[cfg(not(unix))]
fn foreign_to_root(_path: &Path) -> bool {
    false
}

#[cfg(unix)]
fn root_home() -> Option<PathBuf> {
    let entry = unsafe { libc::getpwuid(0) };
    if entry.is_null() {
        return None;
    }
    let dir = unsafe { std::ffi::CStr::from_ptr((*entry).pw_dir) };
    use std::os::unix::ffi::OsStrExt;
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(dir.to_bytes())))
}
#[cfg(not(unix))]
fn root_home() -> Option<PathBuf> {
    None
}

fn resolve(value: Option<PathBuf>, home: PathBuf, fallback: &str) -> PathBuf {
    value
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join(fallback))
}

pub fn config_dir() -> PathBuf {
    base("XDG_CONFIG_HOME", ".config").join("slate")
}

pub fn state_dir() -> PathBuf {
    base("XDG_STATE_HOME", ".local/state")
}

/// Serialises unit tests that change the process environment.
#[cfg(test)]
pub(crate) static TEST_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_requires_absolute_paths_and_defaults_for_empty_values() {
        for value in [None, Some(PathBuf::new()), Some(PathBuf::from("relative"))] {
            assert_eq!(
                resolve(value, "/home/test".into(), ".config"),
                PathBuf::from("/home/test/.config")
            );
        }
        assert_eq!(
            resolve(
                Some("/custom/config".into()),
                "/home/test".into(),
                ".config"
            ),
            PathBuf::from("/custom/config")
        );
    }

    #[cfg(unix)]
    #[test]
    fn only_root_avoids_other_users_directories() {
        let dir = tempfile::tempdir().unwrap();
        let expected = unsafe { libc::geteuid() } == 0 && {
            use std::os::unix::fs::MetadataExt;
            std::fs::metadata(dir.path()).unwrap().uid() != 0
        };
        assert_eq!(foreign_to_root(&dir.path().join("missing/child")), expected);
        assert!(!foreign_to_root(Path::new("/")));
    }
}
