use std::path::PathBuf;

fn base(variable: &str, fallback: &str) -> PathBuf {
    resolve(
        std::env::var_os(variable).map(PathBuf::from),
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()),
        fallback,
    )
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
}
