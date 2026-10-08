//! Reading optional TOML files (languages, tools, snippets, tasks, launch
//! configurations). A missing file is not an error; a file that does not
//! parse is reported instead of silently ignored.
use serde::de::DeserializeOwned;
use std::path::Path;

/// The parsed file, `Ok(None)` when it does not exist, or an error naming
/// the file and the problem.
pub fn read<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
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
    }
}
