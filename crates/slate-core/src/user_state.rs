//! Private, bounded convenience-state files.
use std::path::PathBuf;
pub(crate) fn path(name: &str) -> PathBuf {
    crate::paths::state_dir().join("slate").join(name)
}
pub(crate) fn read<T: serde::de::DeserializeOwned + Default>(name: &str) -> T {
    let path = path(name);
    if std::fs::metadata(&path).is_ok_and(|m| m.len() <= 128 * 1024) {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    } else {
        T::default()
    }
}
