//! Cooperative file-use warnings; save baselines remain the conflict authority.
use crate::{user_state::path as state, App};
use std::{collections::BTreeMap, path::PathBuf};
#[derive(Default)]
pub(crate) struct FileLocks {
    claims: BTreeMap<PathBuf, Option<std::fs::File>>,
}
impl App {
    pub(crate) fn update_file_locks(&mut self) {
        use fs2::FileExt;
        use sha2::{Digest, Sha256};
        if !self.preferences.locking {
            self.file_locks.claims.clear();
            return;
        }
        let paths: std::collections::BTreeSet<_> = self
            .documents
            .values()
            .filter(|d| !d.read_only)
            .filter_map(|d| d.path.as_ref())
            .filter_map(|p| p.canonicalize().ok())
            .collect();
        self.file_locks.claims.retain(|p, _| paths.contains(p));
        let dir = state("file-locks");
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.status = format!("File-use warnings unavailable: {e}");
            return;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        for path in paths {
            if self.file_locks.claims.contains_key(&path) {
                continue;
            }
            let hash = format!("{:x}", Sha256::digest(path.as_os_str().as_encoded_bytes()));
            let mut options = std::fs::OpenOptions::new();
            options.create(true).truncate(false).read(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let claim = options
                .open(dir.join(hash))
                .and_then(|f| f.try_lock_exclusive().map(|_| f));
            let foreign = path.file_name().is_some_and(|name| {
                path.with_file_name(format!(".{}.swp", name.to_string_lossy()))
                    .exists()
            });
            match claim {
                Ok(file) => {
                    self.file_locks.claims.insert(path.clone(), Some(file));
                    if foreign {
                        self.status = format!(
                            "Warning: {} has another editor's swap file; check before saving",
                            path.display()
                        );
                    }
                }
                Err(e) => {
                    self.file_locks.claims.insert(path.clone(), None);
                    self.status=format!("Warning: {} may be open in another Slate instance ({e}); edits remain allowed",path.display());
                }
            }
        }
    }
}
