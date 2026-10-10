//! Cooperative file-use warnings; save baselines remain the conflict authority.
//!
//! Each claimed file has a lock file named by its path's hash. A released
//! claim unlinks its lock file while still holding the lock, and a new claim
//! only counts once the locked file is still the one at the path (otherwise
//! it was released and removed meanwhile, and the claim is retried). Older
//! releases did not check this, so between mixed versions a warning can be
//! missed; that is acceptable for advisory warnings.
use crate::{user_state::path as state, App};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// How often a file another instance holds is claimed again.
const RETRY: Duration = Duration::from_secs(2);

/// A held lock file, removed when the claim ends.
struct Held {
    file: std::fs::File,
    path: PathBuf,
}
impl Drop for Held {
    fn drop(&mut self) {
        // Unlink before unlocking (closing): a claim that opens this file
        // afterwards sees that it is no longer at the path and retries.
        let _ = std::fs::remove_file(&self.path);
        let _ = fs2::FileExt::unlock(&self.file);
    }
}
enum Claim {
    /// Kept for its `Drop`.
    Held(#[allow(dead_code)] Held),
    /// Another instance held it at this time; retried after `RETRY`.
    Busy(Instant),
}
#[derive(Default)]
pub(crate) struct FileLocks {
    claims: BTreeMap<PathBuf, Claim>,
}

fn claim(lock: &Path) -> std::io::Result<Held> {
    use fs2::FileExt;
    for _ in 0..3 {
        let mut options = std::fs::OpenOptions::new();
        options.create(true).truncate(false).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(lock)?;
        file.try_lock_exclusive()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let held = file.metadata()?;
            match std::fs::symlink_metadata(lock) {
                Ok(at) if at.dev() == held.dev() && at.ino() == held.ino() => {}
                // Released and removed between opening and locking.
                _ => continue,
            }
        }
        return Ok(Held {
            file,
            path: lock.to_path_buf(),
        });
    }
    Err(std::io::Error::other("its lock file keeps changing"))
}

impl App {
    pub(crate) fn update_file_locks(&mut self) {
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
            let warned = match self.file_locks.claims.get(&path) {
                Some(Claim::Held(_)) => continue,
                Some(Claim::Busy(at)) if at.elapsed() < RETRY => continue,
                Some(Claim::Busy(_)) => true,
                None => false,
            };
            let hash = format!("{:x}", Sha256::digest(path.as_os_str().as_encoded_bytes()));
            let foreign = path.file_name().is_some_and(|name| {
                path.with_file_name(format!(".{}.swp", name.to_string_lossy()))
                    .exists()
            });
            match claim(&dir.join(hash)) {
                Ok(held) => {
                    self.file_locks
                        .claims
                        .insert(path.clone(), Claim::Held(held));
                    // Shown when first claimed, even after waiting.
                    if foreign {
                        self.status = format!(
                            "Warning: {} has another editor's swap file; check before saving",
                            path.display()
                        );
                    }
                }
                Err(e) => {
                    self.file_locks
                        .claims
                        .insert(path.clone(), Claim::Busy(Instant::now()));
                    // Warn once; later retries stay quiet until it is claimed.
                    if !warned {
                        self.status=format!("Warning: {} may be open in another Slate instance ({e}); edits remain allowed",path.display());
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claims_are_retried_and_released_claims_leave_no_lock_file() {
        let _serial = crate::paths::TEST_ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
        std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
        let path = dir.path().join("shared.txt");
        std::fs::write(&path, "text").unwrap();
        let mut a = App::new(&path).unwrap();
        a.configure("locking", "true").unwrap();
        a.update_file_locks();
        let locks = state("file-locks");
        let count = || std::fs::read_dir(&locks).unwrap().count();
        assert_eq!(count(), 1);
        let mut b = App::new(&path).unwrap();
        b.update_file_locks();
        assert!(b.status.contains("another Slate"), "{}", b.status);
        // Retried quietly while the other instance keeps it.
        b.status.clear();
        let retry = |b: &mut App| {
            for claim in b.file_locks.claims.values_mut() {
                *claim = Claim::Busy(Instant::now() - RETRY);
            }
            b.update_file_locks();
        };
        retry(&mut b);
        assert!(b.status.is_empty(), "{}", b.status);
        assert!(matches!(
            b.file_locks.claims.values().next(),
            Some(Claim::Busy(_))
        ));
        // Released: the lock file goes, and the waiting instance claims it,
        // still warning about another editor's swap file.
        std::fs::write(dir.path().join(".shared.txt.swp"), "vim").unwrap();
        a.file_locks.claims.clear();
        assert_eq!(count(), 0);
        retry(&mut b);
        assert!(matches!(
            b.file_locks.claims.values().next(),
            Some(Claim::Held(_))
        ));
        assert!(b.status.contains("swap file"), "{}", b.status);
        assert_eq!(count(), 1);
        b.file_locks.claims.clear();
        assert_eq!(count(), 0);
    }
}
