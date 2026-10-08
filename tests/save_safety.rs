//! Exercise the real privileged-save entrypoint without invoking sudo or
//! touching system files. Each write is confined to a temporary directory.
use slate_core::fsio::Baseline;
use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
};

fn save(
    path: &std::path::Path,
    baseline: &Baseline,
    bytes: &[u8],
    state: &std::path::Path,
) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_slate"))
        .args([
            std::ffi::OsStr::new("--internal-elevated-save"),
            path.as_os_str(),
        ])
        .env("XDG_STATE_HOME", state)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    writeln!(input, "{}", serde_json::to_string(baseline).unwrap()).unwrap();
    input.write_all(bytes).unwrap();
    drop(input);
    child.wait_with_output().unwrap()
}

#[test]
fn elevated_entrypoint_preserves_links_and_rejects_external_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("original.txt");
    let link = dir.path().join("link.txt");
    let original = b"original\n";
    fs::write(&path, original).unwrap();
    fs::hard_link(&path, &link).unwrap();
    let result = save(
        &path,
        &Baseline::of(original),
        b"changed\n",
        &dir.path().join("state"),
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(fs::read(&link).unwrap(), b"changed\n");
    let result = save(
        &path,
        &Baseline::of(original),
        b"stale edit\n",
        &dir.path().join("state"),
    );
    assert!(!result.status.success());
    assert_eq!(fs::read(&path).unwrap(), b"changed\n");
}
