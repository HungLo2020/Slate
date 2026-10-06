use slate_core::{
    document::Document,
    layout::{Axis, Node, Rect, View},
    terminal::TerminalSession,
    App, Command, Key,
};
use std::{
    fs, thread,
    time::{Duration, Instant},
};
fn key(app: &mut App, name: &str) {
    app.dispatch(Command::Key {
        key: Key {
            key: name.into(),
            ..Default::default()
        },
    });
}
#[test]
fn unicode_edit_undo_redo_and_shared_split_views() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("code.rs");
    fs::write(&path, "a👩‍💻界\n").unwrap();
    let mut app = App::new(&path).unwrap();
    key(&mut app, "Right");
    key(&mut app, "Right");
    key(&mut app, "Backspace");
    assert_eq!(app.documents[&10].text, "a界\n");
    app.dispatch(Command::Undo);
    assert_eq!(app.documents[&10].text, "a👩‍💻界\n");
    app.dispatch(Command::Redo);
    assert_eq!(app.documents[&10].text, "a界\n");
    app.dispatch(Command::Split {
        axis: Axis::Vertical,
        kind: None,
    });
    assert_eq!(app.views.len(), 2);
    app.dispatch(Command::Paste { text: "x".into() });
    assert_eq!(app.documents[&10].text, "ax界\n");
    assert!(app.views.values().all(|v| v.document == 10));
    app.dispatch(Command::Save);
    assert_eq!(fs::read_to_string(path).unwrap(), "ax界\n");
    let snapshot = app.snapshot(160, 40, 1, 1, 1, 3);
    assert_eq!(snapshot.panes.len(), 4);
    assert!(snapshot
        .panes
        .iter()
        .filter(|p| p.kind == "editor")
        .all(|p| p.screen.is_some()));
}
#[test]
fn save_preserves_permissions_and_rejects_external_changes_and_clobber() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file");
    fs::write(&path, "old").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o750)).unwrap();
    let mut doc = Document::open(&path).unwrap();
    doc.replace(0, 3, "new", 0).unwrap();
    doc.save(None).unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o750
    );
    doc.replace(0, 3, "local", 0).unwrap();
    fs::write(&path, "external").unwrap();
    assert!(doc.save(None).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "external");
    assert!(doc.dirty());
    let other = dir.path().join("other");
    fs::write(&other, "keep").unwrap();
    assert!(doc.save(Some(&other)).is_err());
    assert_eq!(fs::read_to_string(other).unwrap(), "keep");
    let copy = dir.path().join("copy");
    doc.save(Some(&copy)).unwrap();
    assert_eq!(fs::read_to_string(copy).unwrap(), "local");
}
#[test]
fn symlink_save_changes_target_without_replacing_link() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    let link = dir.path().join("link");
    fs::write(&target, "old").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let mut doc = Document::open(&link).unwrap();
    doc.replace(0, 3, "new", 0).unwrap();
    doc.save(None).unwrap();
    assert!(fs::symlink_metadata(link).unwrap().file_type().is_symlink());
    assert_eq!(fs::read_to_string(target).unwrap(), "new");
}
#[test]
fn nested_layout_geometry_resize_remove_and_validation() {
    let mut layout = Node::default_layout(11, 12);
    assert!(layout.validate());
    layout.split(2, Axis::Vertical, 6, 7, View::Editor(13));
    let (panes, handles) = layout.arrange(
        Rect {
            x: 0,
            y: 0,
            width: 160,
            height: 50,
        },
        1,
    );
    assert_eq!(panes.len(), 4);
    assert_eq!(handles.len(), 3);
    for a in &panes {
        for b in &panes {
            if a.id != b.id {
                assert!(
                    a.rect.x + a.rect.width <= b.rect.x
                        || b.rect.x + b.rect.width <= a.rect.x
                        || a.rect.y + a.rect.height <= b.rect.y
                        || b.rect.y + b.rect.height <= a.rect.y
                );
            }
        }
    }
    layout.resize(4, 0.3);
    assert!(layout.validate());
    assert!(layout.swap(1, 6));
    assert!(layout.remove(6));
    assert_eq!(layout.panes().len(), 3);
    assert!(!layout.remove(999));
}
fn wait_for(term: &TerminalSession, needle: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let text = term
            .screen()
            .cells
            .iter()
            .flat_map(|l| l.iter())
            .map(|c| c.text.as_str())
            .collect::<String>();
        if text.contains(needle) {
            break;
        }
        assert!(Instant::now() < deadline, "missing {needle:?} in {text:?}");
        thread::sleep(Duration::from_millis(20));
    }
}
#[test]
fn real_pty_shell_color_resize_and_alternate_screen() {
    let dir = tempfile::tempdir().unwrap();
    let mut term = TerminalSession::spawn(dir.path(), None, 24, 80).unwrap();
    term.write(b"printf '\\033[31mSLATE_RED\\033[0m\\n'\r")
        .unwrap();
    wait_for(&term, "SLATE_RED");
    assert!(term
        .screen()
        .cells
        .iter()
        .flatten()
        .any(|c| c.fg == "#bf616a"));
    term.resize(31, 91).unwrap();
    term.write(b"stty size > size.txt\r").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !dir.path().join("size.txt").exists() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        fs::read_to_string(dir.path().join("size.txt"))
            .unwrap()
            .trim(),
        "31 91"
    );
    term.write(b"printf '\\033[?1049h\\033[2J\\033[HSLATE_ALT'\r")
        .unwrap();
    wait_for(&term, "SLATE_ALT");
    term.write(b"printf '\\033[?1049l'\r").unwrap();
}
#[test]
fn close_and_quit_protect_unsaved_work() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(dir.path()).unwrap();
    app.dispatch(Command::Paste {
        text: "unsaved".into(),
    });
    app.dispatch(Command::Quit { force: false });
    assert!(!app.quit);
    app.dispatch(Command::CloseDocument { force: false });
    assert!(app.documents[&10].dirty());
    app.dispatch(Command::Quit { force: true });
    assert!(app.quit);
}

#[test]
fn git_status_stage_diff_commit_and_unstage_in_nested_workspace() {
    use std::process::Command as Process;
    let dir = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        let result = Process::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    run(&["init", "-q"]);
    run(&["config", "user.email", "slate-tests@example.invalid"]);
    run(&["config", "user.name", "Slate Tests"]);
    fs::create_dir(dir.path().join("nested")).unwrap();
    fs::write(dir.path().join("nested/code.rs"), "old\n").unwrap();
    let mut app = App::new(&dir.path().join("nested")).unwrap();
    fn wait_status(app: &mut App, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let s = app.snapshot(160, 40, 1, 1, 1, 3);
            if s.git
                .iter()
                .any(|e| e.path == "nested/code.rs" && e.status.contains(needle))
            {
                break;
            }
            assert!(Instant::now() < deadline, "missing {needle}: {}", s.status);
            thread::sleep(Duration::from_millis(10));
        }
    }
    wait_status(&mut app, "??");
    app.dispatch(Command::GitStage {
        path: "nested/code.rs".into(),
    });
    wait_status(&mut app, "A");
    app.dispatch(Command::GitUnstage {
        path: "nested/code.rs".into(),
    });
    wait_status(&mut app, "??");
    assert!(dir.path().join("nested/code.rs").exists());
    app.dispatch(Command::GitStage {
        path: "nested/code.rs".into(),
    });
    wait_status(&mut app, "A");
    app.dispatch(Command::GitCommit {
        message: "Initial".into(),
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while !Process::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .unwrap()
        .status
        .success()
    {
        app.poll();
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    fs::write(dir.path().join("nested/code.rs"), "new\n").unwrap();
    app.dispatch(Command::Refresh);
    wait_status(&mut app, "M");
    app.dispatch(Command::GitDiff {
        path: "nested/code.rs".into(),
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while !app.documents.values().any(|d| d.text.contains("+new")) {
        app.poll();
        assert!(Instant::now() < deadline, "missing diff: {}", app.status);
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn terminal_can_run_fullscreen_vim_and_htop() {
    // These optional integration checks run when the programs exist on the host.
    let dir = tempfile::tempdir().unwrap();
    if std::process::Command::new("vim.tiny")
        .arg("--version")
        .output()
        .is_ok()
    {
        let mut term =
            TerminalSession::spawn(dir.path(), Some("vim.tiny -Nu NONE -n edited.txt"), 24, 80)
                .unwrap();
        thread::sleep(Duration::from_millis(150));
        term.write(b"iVT_EDITOR_OK\x1b:wq\r").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !term.exited() {
            assert!(Instant::now() < deadline, "Vim failed to exit");
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            fs::read_to_string(dir.path().join("edited.txt")).unwrap(),
            "VT_EDITOR_OK\n"
        );
    }
    if std::process::Command::new("htop")
        .arg("--version")
        .output()
        .is_ok()
    {
        let mut term = TerminalSession::spawn(dir.path(), Some("htop"), 24, 80).unwrap();
        wait_for(&term, "Tasks");
        term.write(b"q").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !term.exited() {
            assert!(Instant::now() < deadline, "htop failed to exit");
            thread::sleep(Duration::from_millis(20));
        }
    }
}
