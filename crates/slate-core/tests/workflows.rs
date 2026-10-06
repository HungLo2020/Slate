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
    wait_app(&mut app, |a| a.status == "Saved");
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
    let deadline = Instant::now() + Duration::from_secs(5);
    while !term
        .screen()
        .cells
        .iter()
        .flatten()
        .any(|c| c.fg == "#bf616a")
    {
        assert!(Instant::now() < deadline, "No red output");
        thread::sleep(Duration::from_millis(10));
    }
    term.resize(31, 91).unwrap();
    term.write(b"stty size > size.txt\r").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while fs::read_to_string(dir.path().join("size.txt"))
        .unwrap_or_default()
        .trim()
        != "31 91"
    {
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

fn wait_app(app: &mut App, check: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        app.poll();
        if check(app) {
            return;
        }
        assert!(Instant::now() < deadline, "Timed out: {}", app.status);
        thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn search_replace_unicode_whole_words_literal_replacement_and_indentation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("code.rs");
    fs::write(&path, "cat CAT scatter 猫猫\n    fn main() {\n}").unwrap();
    let mut app = App::new(&path).unwrap();
    app.dispatch(Command::Search {
        query: "cat".into(),
        case_sensitive: false,
        whole_word: true,
        backward: false,
    });
    assert_eq!(app.views[&11].anchor, Some(0));
    assert_eq!(app.views[&11].cursor, 3);
    app.command_line("find-next");
    assert_eq!(app.views[&11].anchor, Some(4));
    app.command_line("find-previous");
    assert_eq!(app.views[&11].anchor, Some(0));
    app.dispatch(Command::Replace {
        query: "cat".into(),
        replacement: "$1猫".into(),
        all: true,
        case_sensitive: false,
        whole_word: true,
    });
    assert!(app.documents[&10].text.starts_with("$1猫 $1猫 scatter"));
    app.dispatch(Command::Undo);
    assert!(app.documents[&10].text.starts_with("cat CAT scatter"));
    app.dispatch(Command::GoToLine { line: 2 });
    key(&mut app, "End");
    key(&mut app, "Enter");
    assert!(app.documents[&10].text.contains("    fn main() {\n    \n}"));
    app.preferences.indent_width = 2;
    key(&mut app, "Tab");
    assert!(app.documents[&10].text.contains("{\n      \n}"));
    app.dispatch(Command::GoToLine { line: 2 });
    app.dispatch(Command::Indent { outdent: false });
    assert!(app.documents[&10].text.contains("      fn main()"));
    app.dispatch(Command::Indent { outdent: true });
    assert!(app.documents[&10].text.contains("    fn main()"));
    app.dispatch(Command::GoToLine { line: 999 });
    assert!(app.status.starts_with("Error:"));
    app.command_line("prompt-find");
    app.dispatch(Command::UpdatePrompt {
        input: "猫猫".into(),
        replacement: String::new(),
        case_sensitive: true,
        whole_word: false,
    });
    app.dispatch(Command::SubmitPrompt { all: false });
    assert_eq!(
        &app.documents[&10].text[app.views[&11].anchor.unwrap()..app.views[&11].cursor],
        "猫猫"
    );
}
#[test]
fn undo_memory_is_proportional_to_edits_in_a_large_buffer() {
    let mut d = Document::scratch();
    d.text = "x".repeat(8 * 1024 * 1024);
    for _ in 0..100 {
        d.replace(0, 0, "猫", 0).unwrap();
    }
    assert_eq!(d.history_bytes(), 300);
    for _ in 0..100 {
        d.undo(0).unwrap();
    }
    assert_eq!(d.text.len(), 8 * 1024 * 1024);
    for _ in 0..100 {
        d.redo(0).unwrap();
    }
    assert!(d.text.starts_with("猫猫"));
    assert_eq!(d.history_bytes(), 300);
}
#[test]
fn background_save_preserves_newer_edits_and_external_conflicts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file");
    fs::write(&path, "old").unwrap();
    let mut app = App::new(&path).unwrap();
    app.dispatch(Command::SelectAll);
    app.dispatch(Command::Paste {
        text: "saved".into(),
    });
    app.dispatch(Command::Save);
    app.dispatch(Command::Paste {
        text: " newer".into(),
    });
    wait_app(&mut app, |a| a.status.starts_with("Saved"));
    assert_eq!(fs::read_to_string(&path).unwrap(), "saved");
    assert_eq!(app.documents[&10].text, "saved newer");
    assert!(app.dirty());
    fs::write(&path, "external").unwrap();
    app.dispatch(Command::Save);
    wait_app(&mut app, |a| a.status.starts_with("Save failed:"));
    assert_eq!(fs::read_to_string(&path).unwrap(), "external");
    assert!(app.dirty());
    app.dispatch(Command::Open { path: path.clone() });
    wait_app(&mut app, |a| a.status.starts_with("Opened"));
    assert_eq!(app.documents.len(), 1);
    assert_eq!(app.documents[&10].text, "saved newer");
}
#[test]
fn recovery_preserves_unsaved_baselines_layout_and_cursors_with_fresh_shells() {
    use slate_core::workspace::WorkspaceStore;
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let path = dir.path().join("code.rs");
    fs::write(&path, "disk").unwrap();
    let mut app = App::new(&path).unwrap();
    let store = WorkspaceStore::acquire_in(dir.path(), state.path()).unwrap();
    let session = store.path.clone();
    assert!(WorkspaceStore::acquire_in(dir.path(), state.path()).is_err());
    app.attach_workspace(store, false).unwrap();
    app.dispatch(Command::Paste {
        text: "unsaved 猫".into(),
    });
    app.dispatch(Command::Split {
        axis: Axis::Vertical,
        kind: None,
    });
    let focus = app.focus;
    app.flush_workspace().unwrap();
    assert!(session.exists());
    drop(app);
    fs::write(&path, "external").unwrap();
    let mut restored = App::new(dir.path()).unwrap();
    restored
        .attach_workspace(
            WorkspaceStore::acquire_in(dir.path(), state.path()).unwrap(),
            true,
        )
        .unwrap();
    assert_eq!(restored.focus, focus);
    assert_eq!(restored.layout.panes().len(), 4);
    assert_eq!(restored.documents[&10].text, "unsaved 猫disk");
    assert!(restored.dirty());
    assert!(restored
        .views
        .values()
        .all(|v| v.cursor == "unsaved 猫".len()));
    restored.dispatch(Command::Save);
    wait_app(&mut restored, |a| a.status.starts_with("Save failed:"));
    assert_eq!(fs::read_to_string(path).unwrap(), "external");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(session).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
#[test]
fn preferences_remap_shared_shortcuts_and_validate_options() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(dir.path()).unwrap();
    app.preferences
        .editor_keys
        .insert("Ctrl+k".into(), "new".into());
    app.dispatch(Command::Key {
        key: Key {
            key: "k".into(),
            ctrl: true,
            ..Default::default()
        },
    });
    assert_eq!(app.documents.len(), 2);
    app.preferences.indent_width = 0;
    assert!(app.preferences.validate().is_err());
    app.preferences.indent_width = 2;
    app.preferences.theme = "light".into();
    assert!(app.preferences.validate().is_ok());
}
#[test]
fn highlighting_is_shared_and_refreshes_after_editing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("code.rs");
    fs::write(&path, "// comment\nlet value = 42;\n").unwrap();
    let mut app = App::new(&path).unwrap();
    app.dispatch(Command::Split {
        axis: Axis::Vertical,
        kind: None,
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let s = app.snapshot(160, 40, 1, 1, 1, 3);
        let panes = s
            .panes
            .iter()
            .filter(|p| p.kind == "editor")
            .collect::<Vec<_>>();
        let colors = panes[0]
            .screen
            .as_ref()
            .unwrap()
            .cells
            .iter()
            .flatten()
            .map(|c| c.fg.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        if colors.len() > 2 {
            assert!(
                panes[0].screen.as_ref().unwrap().cells[..2]
                    == panes[1].screen.as_ref().unwrap().cells[..2],
                "Shared document token colors differ"
            );
            break;
        }
        assert!(Instant::now() < deadline, "No syntax colors");
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn terminal_mouse_modes_bracketed_paste_and_frozen_scrollback_selection() {
    let dir = tempfile::tempdir().unwrap();
    let probe = dir.path().join("probe.py");
    fs::write(
        &probe,
        r#"import os, tty
from pathlib import Path
tty.setraw(0)
os.write(1,b'\x1b[2J\x1b[HSELECT_ME\r\n\x1b[?1002h\x1b[?1006h\x1b[?2004hREADY')
data=b''
while b'\x1b[201~' not in data:
    data+=os.read(0,4096)
Path('input.bin').write_bytes(data)
"#,
    )
    .unwrap();
    let mut term = TerminalSession::spawn(dir.path(), Some("python3 probe.py"), 24, 80).unwrap();
    wait_for(&term, "READY");
    term.pointer(slate_core::terminal::Pointer {
        row: 2,
        col: 4,
        kind: "press",
        button: 0,
        shift: false,
        ctrl: false,
        alt: false,
    })
    .unwrap();
    term.pointer(slate_core::terminal::Pointer {
        row: 2,
        col: 6,
        kind: "drag",
        button: 0,
        shift: false,
        ctrl: true,
        alt: false,
    })
    .unwrap();
    term.pointer(slate_core::terminal::Pointer {
        row: 2,
        col: 6,
        kind: "release",
        button: 0,
        shift: false,
        ctrl: false,
        alt: false,
    })
    .unwrap();
    term.pointer(slate_core::terminal::Pointer {
        row: 2,
        col: 6,
        kind: "wheel_up",
        button: 0,
        shift: false,
        ctrl: false,
        alt: false,
    })
    .unwrap();
    term.pointer(slate_core::terminal::Pointer {
        row: 0,
        col: 0,
        kind: "press",
        button: 0,
        shift: true,
        ctrl: false,
        alt: false,
    })
    .unwrap();
    term.pointer(slate_core::terminal::Pointer {
        row: 0,
        col: 9,
        kind: "drag",
        button: 0,
        shift: true,
        ctrl: false,
        alt: false,
    })
    .unwrap();
    assert_eq!(term.selection_text(), "SELECT_ME");
    term.paste("PASTE\n猫").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !term.exited() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    let input = fs::read(dir.path().join("input.bin")).unwrap();
    assert_eq!(
        input,
        [
            b"\x1b[<0;5;3M\x1b[<48;7;3M\x1b[<0;7;3m\x1b[<64;7;3M\x1b[200~".as_slice(),
            "PASTE\n猫".as_bytes(),
            b"\x1b[201~"
        ]
        .concat()
    );
}
#[test]
fn terminal_cwd_and_workspace_clean_file_refresh() {
    use slate_core::workspace::WorkspaceStore;
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("nested")).unwrap();
    let path = dir.path().join("clean.rs");
    fs::write(&path, "old").unwrap();
    let mut app = App::new(&path).unwrap();
    app.attach_workspace(
        WorkspaceStore::acquire_in(dir.path(), state.path()).unwrap(),
        false,
    )
    .unwrap();
    app.terminals
        .get_mut(&12)
        .unwrap()
        .write(b"cd nested; printf '\\033]7;file://localhost%s\\007CWD_READY' \"$PWD\"\r")
        .unwrap();
    wait_for(&app.terminals[&12], "CWD_READY");
    #[cfg(target_os = "linux")]
    {
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.terminals[&12].cwd() != dir.path().join("nested") {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
    }
    drop(app);
    fs::write(&path, "new disk content").unwrap();
    let mut app = App::new(dir.path()).unwrap();
    app.attach_workspace(
        WorkspaceStore::acquire_in(dir.path(), state.path()).unwrap(),
        true,
    )
    .unwrap();
    assert_eq!(app.documents[&10].text, "new disk content");
    assert!(!app.dirty());
    #[cfg(target_os = "linux")]
    {
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.terminals[&12].cwd() != dir.path().join("nested") {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
fn real_ssh_session_forwards_input_colors_and_terminal_resize() {
    use std::process::Command as Process;
    if !std::path::Path::new("/usr/sbin/sshd").exists() {
        assert!(
            std::env::var_os("SLATE_REQUIRE_TERMINAL_TOOLS").is_none(),
            "sshd required for CI"
        );
        eprintln!("Skipping SSH integration: sshd is not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    for key in ["host", "client"] {
        assert!(Process::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(dir.path().join(key))
            .status()
            .unwrap()
            .success());
    }
    fs::copy(
        dir.path().join("client.pub"),
        dir.path().join("authorized_keys"),
    )
    .unwrap();
    let host = fs::read_to_string(dir.path().join("host.pub")).unwrap();
    fs::write(dir.path().join("known_hosts"), format!("slate-test {host}")).unwrap();
    fs::write(dir.path().join("sshd_config"),format!("HostKey {0}/host\nAuthorizedKeysFile {0}/authorized_keys\nStrictModes no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPermitRootLogin prohibit-password\nUsePAM no\nLogLevel ERROR\n",dir.path().display())).unwrap();
    let user = String::from_utf8(Process::new("id").arg("-un").output().unwrap().stdout).unwrap();
    let command=format!("ssh -tt -i {0}/client -o BatchMode=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile={0}/known_hosts -o HostKeyAlias=slate-test -o 'ProxyCommand=/usr/sbin/sshd -i -e -f {0}/sshd_config' {1}@localhost 'cd {0}; printf \"\\033[31mSSH_READY\\033[0m\\n\"; read reply; printf \"%s\" \"$reply\" > ssh-input; stty size > ssh-size'",dir.path().display(),user.trim());
    let mut term = TerminalSession::spawn(dir.path(), Some(&command), 24, 80).unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let screen = term.screen();
        let text = screen
            .cells
            .iter()
            .flatten()
            .map(|c| c.text.as_str())
            .collect::<String>();
        if text.contains("SSH_READY") {
            break;
        }
        if text.contains("Operation not permitted")
            || text.contains("Missing privilege separation directory")
        {
            assert!(
                std::env::var_os("SLATE_REQUIRE_TERMINAL_TOOLS").is_none(),
                "SSH environment blocked: {text}"
            );
            eprintln!(
                "Skipping SSH integration: environment blocks sshd privilege separation: {text}"
            );
            return;
        }
        assert!(
            Instant::now() < deadline,
            "SSH did not become ready: {text}"
        );
        thread::sleep(Duration::from_millis(20));
    }
    assert!(term
        .screen()
        .cells
        .iter()
        .flatten()
        .any(|c| c.fg == "#bf616a"));
    term.resize(31, 91).unwrap();
    term.write(b"SSH_EDITED\r").unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    while !term.exited() {
        assert!(Instant::now() < deadline, "SSH did not exit");
        thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        fs::read_to_string(dir.path().join("ssh-input")).unwrap(),
        "SSH_EDITED"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("ssh-size"))
            .unwrap()
            .trim(),
        "31 91"
    );
}
#[test]
fn real_tmux_session_preserves_shell_input_and_resize() {
    use std::process::Command as Process;
    let executable = std::env::var("SLATE_TMUX_BINARY").unwrap_or_else(|_| "tmux".into());
    if Process::new(&executable).arg("-V").output().is_err() {
        assert!(
            std::env::var_os("SLATE_REQUIRE_TERMINAL_TOOLS").is_none(),
            "tmux required for CI"
        );
        eprintln!("Skipping tmux integration: tmux is not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("tmux.sock");
    let probe = Process::new(&executable)
        .arg("-S")
        .arg(&socket)
        .args(["-f", "/dev/null", "new-session", "-d", "-s", "probe"])
        .output()
        .unwrap();
    if !probe.status.success() {
        assert!(
            std::env::var_os("SLATE_REQUIRE_TERMINAL_TOOLS").is_none(),
            "Cannot start tmux: {}",
            String::from_utf8_lossy(&probe.stderr)
        );
        eprintln!(
            "Skipping tmux integration: {}",
            String::from_utf8_lossy(&probe.stderr)
        );
        return;
    }
    let _ = Process::new(&executable)
        .arg("-S")
        .arg(&socket)
        .arg("kill-server")
        .status();
    struct Cleanup(String, std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = Process::new(&self.0)
                .arg("-S")
                .arg(&self.1)
                .arg("kill-server")
                .output();
        }
    }
    let _cleanup = Cleanup(executable.clone(), socket.clone());
    let command = format!(
        "{executable} -S {} -f /dev/null new-session -s slate -c {}",
        socket.display(),
        dir.path().display()
    );
    let mut term = TerminalSession::spawn(dir.path(), Some(&command), 24, 80).unwrap();
    wait_for(&term, "slate");
    term.write(b"printf TMUX_EDITED > tmux-input\r").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while fs::read_to_string(dir.path().join("tmux-input")).unwrap_or_default() != "TMUX_EDITED" {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(20));
    }
    term.resize(31, 91).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let result = Process::new(&executable)
            .arg("-S")
            .arg(&socket)
            .args(["display-message", "-p", "#{window_height} #{window_width}"])
            .output()
            .unwrap();
        if String::from_utf8_lossy(&result.stdout).trim() == "30 91" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "tmux resize: {}",
            String::from_utf8_lossy(&result.stdout)
        );
        thread::sleep(Duration::from_millis(20));
    }
    term.write(b"exit\r").unwrap();
}

#[test]
fn discard_quit_writes_valid_state_and_missing_files_are_recoverable() {
    use slate_core::workspace::WorkspaceStore;
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let path = dir.path().join("file");
    fs::write(&path, "base").unwrap();
    let mut app = App::new(&path).unwrap();
    app.attach_workspace(
        WorkspaceStore::acquire_in(dir.path(), state.path()).unwrap(),
        false,
    )
    .unwrap();
    app.dispatch(Command::Paste {
        text: "a much longer edit".into(),
    });
    app.dispatch(Command::Quit { force: true });
    assert!(app.quit);
    drop(app);
    let mut app = App::new(dir.path()).unwrap();
    app.attach_workspace(
        WorkspaceStore::acquire_in(dir.path(), state.path()).unwrap(),
        true,
    )
    .unwrap();
    assert_eq!(app.documents[&10].text, "base");
    assert!(!app.dirty());
    assert!(app.views.values().all(|v| v.cursor <= 4));
    drop(app);
    fs::remove_file(path).unwrap();
    let mut app = App::new(dir.path()).unwrap();
    app.attach_workspace(
        WorkspaceStore::acquire_in(dir.path(), state.path()).unwrap(),
        true,
    )
    .unwrap();
    assert_eq!(app.documents[&10].text, "base");
    assert!(app.dirty());
    app.dispatch(Command::Quit { force: false });
    assert!(!app.quit);
}
#[test]
fn corrupt_workspace_is_preserved_until_an_explicit_fresh_start() {
    use slate_core::workspace::WorkspaceStore;
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let store = WorkspaceStore::acquire_in(dir.path(), state.path()).unwrap();
    let path = store.path.clone();
    fs::write(&path, "broken checkpoint").unwrap();
    let mut app = App::new(dir.path()).unwrap();
    assert!(app.attach_workspace(store, true).is_err());
    drop(app);
    assert_eq!(fs::read_to_string(&path).unwrap(), "broken checkpoint");
    let mut app = App::new(dir.path()).unwrap();
    app.attach_workspace(
        WorkspaceStore::acquire_in(dir.path(), state.path()).unwrap(),
        false,
    )
    .unwrap();
    app.flush_workspace().unwrap();
    assert!(serde_json::from_slice::<serde_json::Value>(&fs::read(path).unwrap()).is_ok());
}
