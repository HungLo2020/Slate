//! Reading optional TOML files (languages, tools, snippets, tasks, launch
//! configurations). A missing file is not an error; a file that does not
//! parse is reported instead of silently ignored.
use serde::de::DeserializeOwned;
use std::path::Path;

/// Configuration files are small; project ones (`.slate/*.toml`) are read
/// before the folder is trusted, so a FIFO or huge file must not stall or
/// exhaust the editor.
pub(crate) const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

/// The parsed file, `Ok(None)` when it does not exist, or an error naming
/// the file and the problem. Only regular files up to 1 MiB are read.
pub fn read<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    let text = match crate::fsio::read_regular(path, MAX_CONFIG_BYTES) {
        Ok(bytes) => {
            String::from_utf8(bytes).map_err(|_| format!("{}: not valid UTF-8", path.display()))?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::Other => return Err(e.to_string()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    toml::from_str(&text)
        .map(Some)
        .map_err(|e| format!("{}: {}", path.display(), e.message()))
}

/// The entries a file lists (`entries` picks them out of the parsed file),
/// empty when it is missing; a parse error is added to `errors`.
pub fn list<F: DeserializeOwned, T>(
    path: &Path,
    entries: impl FnOnce(F) -> Vec<T>,
    errors: &mut Vec<String>,
) -> Vec<T> {
    match read::<F>(path) {
        Ok(file) => file.map(entries).unwrap_or_default(),
        Err(error) => {
            errors.push(error);
            Vec::new()
        }
    }
}

impl crate::App {
    /// Show the first configuration file that does not parse.
    pub(crate) fn config_errors(&mut self, errors: Vec<String>) {
        if let Some(error) = errors.into_iter().next() {
            self.status = format!("Error in {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, serde::Deserialize)]
    struct File {
        value: u8,
    }

    #[test]
    fn missing_files_are_empty_and_broken_ones_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.toml");
        assert!(read::<File>(&path).unwrap().is_none());
        std::fs::write(&path, "value = 3\n").unwrap();
        assert_eq!(read::<File>(&path).unwrap().unwrap().value, 3);
        std::fs::write(&path, "value = \"three\"\n").unwrap();
        let error = read::<File>(&path).unwrap_err();
        assert!(error.starts_with(&path.display().to_string()), "{error}");
        assert!(error.contains("invalid type"), "{error}");
        std::fs::write(&path, vec![b'#'; MAX_CONFIG_BYTES as usize + 1]).unwrap();
        assert!(read::<File>(&path).unwrap_err().contains("larger than"));
    }

    #[cfg(unix)]
    #[test]
    fn project_configuration_never_blocks_on_a_fifo() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let fifo = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let error = read::<File>(&path).unwrap_err();
        assert!(error.contains("not a regular file"), "{error}");
    }
}
