//! Saving files without losing the metadata other tools expect: permissions,
//! owner and group, extended attributes (including ACLs) and hard links.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::Command,
};

/// The bytes a document was loaded from or last written to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Baseline {
    pub hash: String,
    pub len: u64,
}
impl Baseline {
    pub fn of(bytes: &[u8]) -> Self {
        Self {
            hash: format!("{:x}", Sha256::digest(bytes)),
            len: bytes.len() as u64,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SaveErrorKind {
    /// The file is read-only for its owner; a confirmed save may replace it.
    ReadOnly,
    /// The user lacks permission; an elevated (sudo/pkexec) save can write it.
    PermissionDenied,
    /// The file changed on disk since it was read.
    Conflict,
    Other,
}
#[derive(Clone, Debug)]
pub struct SaveError {
    pub kind: SaveErrorKind,
    pub message: String,
}
impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for SaveError {}
pub(crate) fn save_error(kind: SaveErrorKind, message: impl Into<String>) -> anyhow::Error {
    SaveError {
        kind,
        message: message.into(),
    }
    .into()
}
pub fn error_kind(error: &anyhow::Error) -> SaveErrorKind {
    if let Some(e) = error.downcast_ref::<SaveError>() {
        return e.kind;
    }
    match error.downcast_ref::<std::io::Error>() {
        Some(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            SaveErrorKind::PermissionDenied
        }
        _ => SaveErrorKind::Other,
    }
}

#[derive(Clone, Copy, Default)]
pub struct WriteOptions {
    /// Replace a file whose permission bits forbid writing (after confirmation).
    pub allow_read_only: bool,
    /// Keep the previous contents as `NAME~`.
    pub backup: bool,
}

#[cfg(unix)]
fn accessible(path: &Path, mode: libc::c_int) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    unsafe { libc::access(path.as_ptr(), mode) == 0 }
}
#[cfg(not(unix))]
fn accessible(path: &Path, _mode: i32) -> bool {
    fs::metadata(path).is_ok_and(|m| !m.permissions().readonly())
}
/// Whether the current user may write this existing file.
pub fn writable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        accessible(path, libc::W_OK)
    }
    #[cfg(not(unix))]
    {
        accessible(path, 0)
    }
}

pub fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push("~");
    path.with_file_name(name)
}

fn check_baseline(path: &Path, baseline: Option<&Baseline>) -> Result<()> {
    match (baseline, fs::read(path)) {
        (Some(expected), Ok(current)) if Baseline::of(&current) != *expected => Err(save_error(
            SaveErrorKind::Conflict,
            "File changed on disk. Reload it, or Save As to a new path to keep both versions",
        )),
        (Some(_), Err(e)) if e.kind() == std::io::ErrorKind::NotFound => Err(save_error(
            SaveErrorKind::Conflict,
            "File was removed externally. Save As to a new path",
        )),
        (None, Ok(_)) => Err(save_error(
            SaveErrorKind::Conflict,
            "A file was created at this path after it was opened. Reload it or Save As",
        )),
        (_, Err(e)) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// Write `bytes` to `path`. `baseline` is the content the caller last saw
/// (`None` means the file must not exist yet). Returns the new baseline.
pub fn write_file(
    path: &Path,
    bytes: &[u8],
    baseline: Option<&Baseline>,
    options: WriteOptions,
) -> Result<Baseline> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if !parent.is_dir() {
        return Err(save_error(
            SaveErrorKind::Other,
            format!("Directory {} does not exist", parent.display()),
        ));
    }
    let existing = fs::metadata(path).ok();
    if let Some(meta) = &existing {
        if !meta.is_file() {
            return Err(save_error(SaveErrorKind::Other, "Not a regular file"));
        }
        if !writable(path) {
            #[cfg(unix)]
            let owned = {
                use std::os::unix::fs::MetadataExt;
                meta.uid() == unsafe { libc::geteuid() }
            };
            #[cfg(not(unix))]
            let owned = true;
            if !owned {
                return Err(save_error(
                    SaveErrorKind::PermissionDenied,
                    format!(
                        "Permission denied: {} belongs to another user",
                        path.display()
                    ),
                ));
            }
            if !options.allow_read_only {
                return Err(save_error(
                    SaveErrorKind::ReadOnly,
                    format!("{} is read-only", path.display()),
                ));
            }
        }
    }
    check_baseline(path, baseline)?;
    if options.backup && existing.is_some() {
        let backup = backup_path(path);
        let temp = tempfile::NamedTempFile::new_in(parent)?;
        fs::copy(path, temp.path())
            .with_context(|| format!("Could not write backup {}", backup.display()))?;
        temp.as_file().sync_all()?;
        temp.persist(&backup).map_err(|e| e.error)?;
        sync_directory(parent);
    }
    let Some(meta) = existing else {
        return write_new(path, parent, bytes, baseline);
    };
    #[cfg(unix)]
    let linked = {
        use std::os::unix::fs::MetadataExt;
        meta.nlink() > 1
    };
    #[cfg(not(unix))]
    let linked = false;
    #[cfg(unix)]
    let directory_writable = accessible(parent, libc::W_OK);
    #[cfg(not(unix))]
    let directory_writable = true;
    // Hard links and directories we may not modify can only keep their
    // identity through an in-place write. Otherwise replace atomically.
    if !linked && directory_writable {
        match write_atomic(path, parent, bytes, &meta, baseline) {
            Ok(written) => return Ok(written),
            Err(AtomicFailure::Metadata) => {}
            Err(AtomicFailure::Error(e)) => return Err(e),
        }
    }
    write_in_place(path, bytes, baseline, options.allow_read_only, &meta)
}

fn map_io(error: std::io::Error, path: &Path) -> anyhow::Error {
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        save_error(
            SaveErrorKind::PermissionDenied,
            format!("Permission denied writing {}", path.display()),
        )
    } else {
        anyhow::Error::new(error).context(format!("Could not write {}", path.display()))
    }
}

fn write_new(
    path: &Path,
    parent: &Path,
    bytes: &[u8],
    baseline: Option<&Baseline>,
) -> Result<Baseline> {
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| map_io(e, path))?;
    #[cfg(unix)]
    {
        // NamedTempFile is private (0600). A new document gets the usual umask mode.
        use std::os::unix::fs::PermissionsExt;
        let umask = unsafe {
            let mask = libc::umask(0o022);
            libc::umask(mask);
            mask
        };
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o666 & !(umask as u32)))?;
    }
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    check_baseline(path, baseline)?;
    temp.persist_noclobber(path).map_err(|e| {
        if e.error.kind() == std::io::ErrorKind::AlreadyExists {
            save_error(
                SaveErrorKind::Conflict,
                "A file was created at this path during the save",
            )
        } else {
            map_io(e.error, path)
        }
    })?;
    sync_directory(parent);
    Ok(Baseline::of(bytes))
}

enum AtomicFailure {
    /// The replacement could not carry the original owner or attributes.
    Metadata,
    Error(anyhow::Error),
}

fn write_atomic(
    path: &Path,
    parent: &Path,
    bytes: &[u8],
    meta: &fs::Metadata,
    baseline: Option<&Baseline>,
) -> std::result::Result<Baseline, AtomicFailure> {
    let fail = AtomicFailure::Error;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| fail(map_io(e, path)))?;
    temp.as_file()
        .set_permissions(meta.permissions())
        .map_err(|e| fail(e.into()))?;
    #[cfg(unix)]
    {
        use std::os::unix::{fs::MetadataExt, io::AsRawFd};
        let (uid, gid) = unsafe { (libc::geteuid(), libc::getegid()) };
        if meta.uid() != uid || meta.gid() != gid {
            // Root keeps a user's file owned by that user; a group member keeps its group.
            if unsafe { libc::fchown(temp.as_file().as_raw_fd(), meta.uid(), meta.gid()) } != 0 {
                return Err(AtomicFailure::Metadata);
            }
            // fchown can clear set-id bits; restore the original mode.
            temp.as_file()
                .set_permissions(meta.permissions())
                .map_err(|e| fail(e.into()))?;
        }
        if !copy_xattrs(path, temp.as_file()) {
            return Err(AtomicFailure::Metadata);
        }
    }
    temp.write_all(bytes).map_err(|e| fail(e.into()))?;
    temp.as_file().sync_all().map_err(|e| fail(e.into()))?;
    // Recheck immediately before replacement; edits made elsewhere are never overwritten.
    check_baseline(path, baseline).map_err(fail)?;
    temp.persist(path)
        .map_err(|e| fail(map_io(e.error, path)))?;
    sync_directory(parent);
    Ok(Baseline::of(bytes))
}

fn write_in_place(
    path: &Path,
    bytes: &[u8],
    baseline: Option<&Baseline>,
    allow_read_only: bool,
    meta: &fs::Metadata,
) -> Result<Baseline> {
    let restore = if allow_read_only && !writable(path) {
        // The owner confirmed replacing a read-only file. Make it writable
        // only for the duration of the write.
        let mut permissions = meta.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions).map_err(|e| map_io(e, path))?;
        Some(meta.permissions())
    } else {
        None
    };
    let result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|e| map_io(e, path))?;
        check_baseline(path, baseline)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let current = fs::metadata(path)?;
            let opened = file.metadata()?;
            anyhow::ensure!(
                current.dev() == opened.dev() && current.ino() == opened.ino(),
                "File replaced while saving; nothing was changed"
            );
        }
        // In-place writes preserve hard links and metadata, but cannot be
        // atomic. Persist a private original before modifying the inode.
        let recovery_dir = crate::paths::state_dir().join("slate/save-recovery");
        fs::create_dir_all(&recovery_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&recovery_dir, fs::Permissions::from_mode(0o700))?;
        }
        let prefix = format!(
            "{}-",
            path.file_name().unwrap_or_default().to_string_lossy()
        );
        let mut original = tempfile::Builder::new()
            .prefix(&prefix)
            .tempfile_in(&recovery_dir)?;
        std::io::copy(&mut file, &mut original)?;
        original.as_file().sync_all()?;
        check_baseline(path, baseline)?;
        let (mut original, recovery_path) = original.keep().map_err(|e| e.error)?;
        sync_directory(&recovery_dir);
        let write = (|| -> Result<()> {
            file.seek(SeekFrom::Start(0))?;
            file.set_len(0)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            Ok(())
        })();
        if let Err(error) = write {
            let restored = (|| -> Result<()> {
                original.seek(SeekFrom::Start(0))?;
                file.seek(SeekFrom::Start(0))?;
                file.set_len(0)?;
                std::io::copy(&mut original, &mut file)?;
                file.sync_all()?;
                Ok(())
            })();
            if let Err(rollback) = restored {
                anyhow::bail!("Save failed: {error:#}; restore failed: {rollback:#}. Original bytes preserved in {}", recovery_path.display());
            }
            let _ = fs::remove_file(&recovery_path);
            return Err(error.context("Save failed; original contents restored"));
        }
        let _ = fs::remove_file(recovery_path);
        Ok(())
    })();
    if let Some(permissions) = restore {
        let _ = fs::set_permissions(path, permissions);
    }
    result?;
    Ok(Baseline::of(bytes))
}

fn sync_directory(parent: &Path) {
    #[cfg(unix)]
    if let Ok(dir) = fs::File::open(parent) {
        let _ = dir.sync_all();
    }
}

/// Copy extended attributes (ACLs, SELinux labels, user metadata). Returns
/// false when an attribute that the original carries cannot be reproduced.
#[cfg(target_os = "linux")]
fn copy_xattrs(source: &Path, target: &fs::File) -> bool {
    use std::os::unix::{ffi::OsStrExt, io::AsRawFd};
    let Ok(path) = std::ffi::CString::new(source.as_os_str().as_bytes()) else {
        return true;
    };
    let size = unsafe { libc::listxattr(path.as_ptr(), std::ptr::null_mut(), 0) };
    if size <= 0 {
        return true;
    }
    let mut names = vec![0u8; size as usize];
    let size = unsafe { libc::listxattr(path.as_ptr(), names.as_mut_ptr().cast(), names.len()) };
    if size <= 0 {
        return true;
    }
    names.truncate(size as usize);
    for name in names.split(|b| *b == 0).filter(|n| !n.is_empty()) {
        let Ok(name) = std::ffi::CString::new(name) else {
            continue;
        };
        let length =
            unsafe { libc::getxattr(path.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0) };
        if length < 0 {
            continue;
        }
        let mut value = vec![0u8; length as usize];
        let length = unsafe {
            libc::getxattr(
                path.as_ptr(),
                name.as_ptr(),
                value.as_mut_ptr().cast(),
                value.len(),
            )
        };
        if length < 0 {
            continue;
        }
        let result = unsafe {
            libc::fsetxattr(
                target.as_raw_fd(),
                name.as_ptr(),
                value.as_ptr().cast(),
                length as usize,
                0,
            )
        };
        if result != 0 {
            return false;
        }
    }
    true
}
#[cfg(all(unix, not(target_os = "linux")))]
fn copy_xattrs(_source: &Path, _target: &fs::File) -> bool {
    true
}

/// Replace an application file (settings, layouts, trust, recovery)
/// atomically: a private temporary file in the same folder, synced, then
/// renamed over the old one. The folder is created when missing.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    sync_directory(dir);
    Ok(())
}

/// The privileged subprocess uses the same conflict checks, atomic writes,
/// metadata preservation and rollback as an ordinary save. Never truncate via tee.
pub fn write_elevated(
    path: &Path,
    bytes: &[u8],
    baseline: Option<&Baseline>,
    program: &str,
    interactive: bool,
) -> Result<()> {
    let executable = std::env::current_exe()?;
    let mut command = Command::new(program);
    if program.ends_with("sudo") {
        command.arg("--");
    }
    command
        .arg(executable)
        .arg("--internal-elevated-save")
        .arg(path);
    let mut input = serde_json::to_vec(&baseline)?;
    input.push(b'\n');
    input.extend_from_slice(bytes);
    let timeout = std::time::Duration::from_secs(120);
    let result = if interactive {
        crate::process::run_interactive(&mut command, input, timeout, program)
    } else {
        crate::process::run(&mut command, Some(input), timeout, program)
    }
    .map_err(anyhow::Error::msg)?;
    anyhow::ensure!(
        result.status.success(),
        "{program} could not save {}: {}",
        path.display(),
        String::from_utf8_lossy(&result.stderr).trim()
    );
    Ok(())
}

/// Recognized before parsing ordinary frontend arguments or loading config.
/// Its destination is visible in sudo/polkit's command; input carries the
/// expected disk baseline followed by the proposed file bytes.
pub fn elevated_save_entry(args: &[std::ffi::OsString]) -> Option<Result<()>> {
    if args
        .first()
        .is_none_or(|arg| arg != "--internal-elevated-save")
    {
        return None;
    }
    Some((|| {
        use std::io::BufRead;
        anyhow::ensure!(args.len() == 2, "Expected one elevated-save destination");
        let path = Path::new(&args[1]);
        anyhow::ensure!(
            path.is_absolute(),
            "Elevated-save destination must be absolute"
        );
        let mut input = std::io::BufReader::new(std::io::stdin().lock());
        let mut header = String::new();
        input.by_ref().take(4096).read_line(&mut header)?;
        anyhow::ensure!(header.ends_with('\n'), "Invalid elevated-save header");
        let baseline: Option<Baseline> = serde_json::from_str(&header)?;
        let mut bytes = Vec::new();
        input
            .take(crate::document::MAX_FILE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        anyhow::ensure!(
            bytes.len() <= crate::document::MAX_FILE_BYTES,
            "Document is too large"
        );
        write_file(
            path,
            &bytes,
            baseline.as_ref(),
            WriteOptions {
                allow_read_only: true,
                backup: false,
            },
        )?;
        Ok(())
    })())
}

pub fn which(program: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn failed_inplace_saves_restore_data() {
        if let Some(directory) = std::env::var_os("SLATE_TEST_SAVE_LIMIT") {
            let directory = PathBuf::from(directory);
            let path = directory.join("original.txt");
            let original = fs::read(&path).unwrap();
            unsafe {
                libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
                let limit = libc::rlimit {
                    rlim_cur: 64,
                    rlim_max: 64,
                };
                assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &limit), 0);
            }
            let result = write_file(
                &path,
                &[b'X'; 200],
                Some(&Baseline::of(&original)),
                WriteOptions::default(),
            );
            assert!(result.is_err());
            assert_eq!(fs::read(&path).unwrap(), original);
            assert_eq!(fs::read(directory.join("link.txt")).unwrap(), original);
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("original.txt");
        fs::write(&path, b"Original bytes\n").unwrap();
        fs::hard_link(&path, dir.path().join("link.txt")).unwrap();
        let result = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "fsio::tests::failed_inplace_saves_restore_data"])
            .env("SLATE_TEST_SAVE_LIMIT", dir.path())
            .env("XDG_STATE_HOME", dir.path().join("state"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[test]
    fn new_files_conflicts_and_backups() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        let first = write_file(&path, b"one\n", None, WriteOptions::default()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"one\n");
        // A second "new file" save must not clobber what now exists.
        let error = write_file(&path, b"x", None, WriteOptions::default()).unwrap_err();
        assert_eq!(error_kind(&error), SaveErrorKind::Conflict);
        fs::write(&path, b"external\n").unwrap();
        let error = write_file(&path, b"two\n", Some(&first), WriteOptions::default()).unwrap_err();
        assert_eq!(error_kind(&error), SaveErrorKind::Conflict);
        let current = Baseline::of(b"external\n");
        write_file(
            &path,
            b"two\n",
            Some(&current),
            WriteOptions {
                backup: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(fs::read(backup_path(&path)).unwrap(), b"external\n");
        assert_eq!(fs::read(&path).unwrap(), b"two\n");
    }

    #[cfg(unix)]
    #[test]
    fn backup_replaces_links_without_overwriting_their_targets() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("document.txt");
        let other = dir.path().join("other.txt");
        fs::write(&other, b"keep").unwrap();
        for hard_link in [false, true] {
            fs::write(&path, b"original").unwrap();
            let backup = backup_path(&path);
            let _ = fs::remove_file(&backup);
            if hard_link {
                fs::hard_link(&other, &backup).unwrap();
            } else {
                symlink(&other, &backup).unwrap();
            }
            write_file(
                &path,
                b"updated",
                Some(&Baseline::of(b"original")),
                WriteOptions {
                    backup: true,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(fs::read(&other).unwrap(), b"keep");
            assert_eq!(fs::read(&backup).unwrap(), b"original");
            assert!(!fs::symlink_metadata(&backup)
                .unwrap()
                .file_type()
                .is_symlink());
        }
    }

    #[cfg(unix)]
    #[test]
    fn hard_links_permissions_and_read_only_files_are_respected() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let _env = crate::paths::TEST_ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
        let path = dir.path().join("linked.txt");
        let link = dir.path().join("other-name.txt");
        fs::write(&path, b"old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        fs::hard_link(&path, &link).unwrap();
        let inode = fs::metadata(&path).unwrap().ino();
        write_file(
            &path,
            b"new",
            Some(&Baseline::of(b"old")),
            WriteOptions::default(),
        )
        .unwrap();
        assert_eq!(
            fs::read(&link).unwrap(),
            b"new",
            "hard link must see the edit"
        );
        assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
        assert_eq!(fs::metadata(&path).unwrap().nlink(), 2);
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o640);

        let ro = dir.path().join("ro.txt");
        fs::write(&ro, b"keep").unwrap();
        fs::set_permissions(&ro, fs::Permissions::from_mode(0o444)).unwrap();
        if unsafe { libc::geteuid() } != 0 {
            let error = write_file(
                &ro,
                b"x",
                Some(&Baseline::of(b"keep")),
                WriteOptions::default(),
            )
            .unwrap_err();
            assert_eq!(error_kind(&error), SaveErrorKind::ReadOnly);
            assert_eq!(fs::read(&ro).unwrap(), b"keep");
        }
        write_file(
            &ro,
            b"changed",
            Some(&Baseline::of(b"keep")),
            WriteOptions {
                allow_read_only: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(fs::read(&ro).unwrap(), b"changed");
        assert_eq!(fs::metadata(&ro).unwrap().mode() & 0o777, 0o444);
    }

    #[cfg(unix)]
    #[test]
    fn unwritable_directories_fall_back_to_in_place_writes() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return; // root bypasses directory permissions
        }
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        fs::create_dir(&locked).unwrap();
        let path = locked.join("file.txt");
        fs::write(&path, b"a").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
        let result = write_file(
            &path,
            b"b",
            Some(&Baseline::of(b"a")),
            WriteOptions::default(),
        );
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        result.unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"b");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn extended_attributes_survive_atomic_replacement() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.txt");
        fs::write(&path, b"a").unwrap();
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        let name = std::ffi::CString::new("user.slate-test").unwrap();
        let set =
            unsafe { libc::setxattr(c_path.as_ptr(), name.as_ptr(), b"v".as_ptr().cast(), 1, 0) };
        if set != 0 {
            return; // filesystem without user xattrs (e.g. some tmpfs builds)
        }
        write_file(
            &path,
            b"b",
            Some(&Baseline::of(b"a")),
            WriteOptions::default(),
        )
        .unwrap();
        let mut value = [0u8; 4];
        let length =
            unsafe { libc::getxattr(c_path.as_ptr(), name.as_ptr(), value.as_mut_ptr().cast(), 4) };
        assert_eq!(&value[..length as usize], b"v");
    }
}
