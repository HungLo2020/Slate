//! Git and workspace trust: restricted folders never run repository
//! programs, trusting asks once and then proceeds, and files whose names are
//! not UTF-8 can still be staged.
use slate_core::{App, Command};
use std::{fs, path::Path, process, thread, time::Duration};

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn isolated() -> (tempfile::TempDir, std::sync::MutexGuard<'static, ()>) {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
    std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
    (dir, guard)
}
fn git(root: &Path, args: &[&str]) {
    let status = process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "user.name=Slate",
            "-c",
            "user.email=slate@example.com",
        ])
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}
fn repository(dir: &Path) -> std::path::PathBuf {
    let root = dir.join("repo");
    fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    root
}
fn settle(app: &mut App) {
    for _ in 0..800 {
        app.process_events();
        let frame = app.snapshot(120, 40, 1, 1, 1, 3);
        if !frame.git_busy {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("Git did not settle: {}", app.status);
}
fn staged(root: &Path) -> String {
    let out = process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["diff", "--cached", "--name-only", "-z"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn restricted_folders_ask_before_running_repository_programs() {
    let (dir, _serial) = isolated();
    let root = repository(dir.path());
    // A repository-configured fsmonitor would run on every status.
    let marker = dir.path().join("fsmonitor-ran");
    let hook = dir.path().join("monitor.sh");
    fs::write(
        &hook,
        format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display()),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    git(&root, &["config", "core.fsmonitor", hook.to_str().unwrap()]);
    fs::write(root.join("a.txt"), "a").unwrap();

    let mut app = App::new(&root).unwrap();
    assert!(!app.trusted());
    app.dispatch(Command::Refresh);
    settle(&mut app);
    assert!(
        !marker.exists(),
        "fsmonitor must not run in restricted mode"
    );
    assert_eq!(app.snapshot(120, 40, 1, 1, 1, 3).git.len(), 1);

    // Staging asks first; declining leaves the index alone.
    app.dispatch(Command::GitStage {
        path: "a.txt".into(),
    });
    assert_eq!(app.prompt.as_ref().unwrap().kind, "trust");
    app.dispatch(Command::DismissPrompt);
    settle(&mut app);
    assert!(staged(&root).is_empty());
    assert!(app.status.starts_with("Restricted mode"), "{}", app.status);

    // Accepting trusts the folder and runs the staging that asked.
    app.dispatch(Command::GitStage {
        path: "a.txt".into(),
    });
    app.dispatch(Command::SubmitPrompt { all: false });
    settle(&mut app);
    assert_eq!(staged(&root), "a.txt\0");
    assert!(app.trusted());
    drop(app);
    // Trust is remembered for the folder.
    assert!(App::new(&root).unwrap().trusted());
    assert!(slate_core::trust::is_trusted(&root));
}

#[cfg(unix)]
#[test]
fn files_with_non_utf8_names_can_be_staged_and_unstaged() {
    use std::os::unix::ffi::OsStrExt;
    let (dir, _serial) = isolated();
    let root = repository(dir.path());
    let name = std::ffi::OsStr::from_bytes(b"caf\xe9.txt");
    fs::write(root.join(name), "bytes").unwrap();
    let mut app = App::new(&root).unwrap();
    app.set_session_trust(true);
    settle(&mut app);
    let path = app.snapshot(120, 40, 1, 1, 1, 3).git[0].path.clone();
    assert_eq!(path, "caf\u{fffd}.txt");
    app.dispatch(Command::GitStage { path: path.clone() });
    settle(&mut app);
    assert_eq!(staged(&root).as_bytes(), "caf\u{fffd}.txt\0".as_bytes());
    let out = process::Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["diff", "--cached", "--name-only", "-z"])
        .output()
        .unwrap();
    assert_eq!(out.stdout, b"caf\xe9.txt\0", "the real bytes were staged");
    app.dispatch(Command::GitUnstage { path });
    settle(&mut app);
    assert!(staged(&root).is_empty(), "{}", app.status);
}
