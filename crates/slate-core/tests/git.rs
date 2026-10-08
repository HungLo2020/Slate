//! Git and workspace trust: restricted folders never run repository
//! programs, trusting asks once and then proceeds, and files whose names are
//! not UTF-8 can still be staged.
mod common;
use slate_core::{App, Command};
use std::{fs, path::Path, process, thread, time::Duration};

use common::isolated;
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
fn restricted_folders_run_no_git_until_trusted() {
    let (dir, _serial) = isolated();
    let root = repository(dir.path());
    // Repository configuration can name programs: an fsmonitor that runs on
    // every status, and a clean filter that runs when Git looks at a file.
    let marker = dir.path().join("program-ran");
    let program = dir.path().join("program.sh");
    fs::write(
        &program,
        format!("#!/bin/sh\ntouch '{}'\ncat\n", marker.display()),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    git(
        &root,
        &["config", "core.fsmonitor", program.to_str().unwrap()],
    );
    git(
        &root,
        &["config", "filter.evil.clean", program.to_str().unwrap()],
    );
    fs::write(root.join(".gitattributes"), "*.txt filter=evil\n").unwrap();
    fs::write(root.join("a.txt"), "a").unwrap();

    let mut app = App::new(&root).unwrap();
    assert!(!app.trusted());
    app.dispatch(Command::Refresh);
    settle(&mut app);
    assert!(
        !marker.exists(),
        "no repository program runs in restricted mode"
    );
    let frame = app.snapshot(120, 40, 1, 1, 1, 3);
    assert!(frame.git_restricted && frame.git.is_empty());
    assert!(frame.git_error.contains("trust"), "{}", frame.git_error);

    // Staging asks first; declining runs nothing.
    app.dispatch(Command::GitStage {
        path: "a.txt".into(),
    });
    assert_eq!(app.prompt.as_ref().unwrap().kind, "trust");
    app.dispatch(Command::DismissPrompt);
    settle(&mut app);
    assert!(!marker.exists());
    assert!(app.status.starts_with("Restricted mode"), "{}", app.status);

    // Accepting trusts the folder and runs the staging that asked.
    app.dispatch(Command::GitStage {
        path: "a.txt".into(),
    });
    app.dispatch(Command::SubmitPrompt { all: false });
    settle(&mut app);
    assert!(staged(&root).contains("a.txt"), "{:?}", staged(&root));
    assert!(app.trusted());
    let frame = app.snapshot(120, 40, 1, 1, 1, 3);
    assert!(!frame.git_restricted);
    drop(app);
    // Trust is remembered for the folder.
    assert!(App::new(&root).unwrap().trusted());
    assert!(slate_core::trust::is_trusted(&root));
}

#[test]
fn a_repository_larger_than_the_workspace_needs_its_own_trust_and_stays_scoped() {
    let (dir, _serial) = isolated();
    let root = repository(dir.path());
    let workspace = root.join("project");
    fs::create_dir_all(&workspace).unwrap();
    fs::write(root.join("secret.txt"), "outside").unwrap();
    fs::write(workspace.join("inside.txt"), "inside").unwrap();
    slate_core::trust::set_trusted(&workspace, true).unwrap();
    let mut app = App::new(&workspace).unwrap();
    assert!(app.trusted());
    settle(&mut app);
    // The repository's folder (above the workspace) is not trusted.
    assert!(app.snapshot(120, 40, 1, 1, 1, 3).git_restricted);
    app.dispatch(Command::GitStageAll);
    let prompt = app.prompt.clone().unwrap();
    assert_eq!(prompt.kind, "trust");
    assert!(
        prompt
            .input
            .starts_with(&root.canonicalize().unwrap().display().to_string()),
        "{}",
        prompt.input
    );
    app.dispatch(Command::SubmitPrompt { all: false });
    settle(&mut app);
    // Only the workspace's files are listed and staged.
    let paths: Vec<String> = app
        .snapshot(120, 40, 1, 1, 1, 3)
        .git
        .iter()
        .map(|e| e.path.clone())
        .collect();
    assert!(paths.iter().all(|p| !p.contains("secret")), "{paths:?}");
    assert!(
        staged(&root).contains("project/inside.txt"),
        "{:?}",
        staged(&root)
    );
    assert!(!staged(&root).contains("secret"));
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

#[test]
fn trusting_while_editing_a_single_file_lasts_for_the_session_only() {
    let (dir, _serial) = isolated();
    let folder = dir.path().join("Downloads");
    fs::create_dir_all(&folder).unwrap();
    let file = folder.join("notes.md");
    fs::write(&file, "notes").unwrap();
    let mut app = App::new(&file).unwrap();
    app.dispatch(Command::Action {
        name: "request-trust".into(),
        argument: String::new(),
    });
    let prompt = app.prompt.clone().unwrap();
    assert!(
        prompt.input.contains("(for this session)"),
        "{}",
        prompt.input
    );
    app.dispatch(Command::SubmitPrompt { all: false });
    assert!(app.trusted());
    assert!(
        !slate_core::trust::is_trusted(&folder),
        "nothing was remembered"
    );
}
