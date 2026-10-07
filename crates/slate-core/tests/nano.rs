//! Shared-core behaviour behind the nano-style workflow: launching files,
//! file formats, saving, quitting, nano editing actions and soft wrap.
use slate_core::{
    cli::{Launch, LaunchFile},
    Command, Key,
};
use std::{fs, path::Path, thread, time::Duration};

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
/// Settings live in process-wide XDG directories, so tests run one at a time.
fn isolated() -> (tempfile::TempDir, std::sync::MutexGuard<'static, ()>) {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    // Each test process gets private configuration and state directories.
    std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
    std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
    (dir, guard)
}
fn launch(files: &[(&Path, Option<usize>)]) -> slate_core::App {
    let launch = Launch {
        recover: true,
        files: files
            .iter()
            .map(|(path, line)| LaunchFile {
                path: path.to_path_buf(),
                line: *line,
                column: None,
            })
            .collect(),
        ..Default::default()
    };
    slate_core::App::launch(&launch, None).unwrap()
}
fn key(app: &mut slate_core::App, name: &str) {
    app.dispatch(Command::Key {
        key: Key {
            key: name.into(),
            ..Default::default()
        },
    });
}
fn ctrl(app: &mut slate_core::App, name: &str) {
    app.dispatch(Command::Key {
        key: Key {
            key: name.into(),
            ctrl: true,
            ..Default::default()
        },
    });
}
fn typed(app: &mut slate_core::App, text: &str) {
    for c in text.chars() {
        app.dispatch(Command::Key {
            key: Key {
                key: c.to_string(),
                text: c.to_string(),
                ..Default::default()
            },
        });
    }
}
fn action(app: &mut slate_core::App, name: &str, argument: &str) {
    app.dispatch(Command::Action {
        name: name.into(),
        argument: argument.into(),
    });
}
fn text(app: &slate_core::App) -> String {
    let view = &app.views[&focused_view(app)];
    app.documents[&view.document].text().to_string()
}
fn focused_view(app: &slate_core::App) -> u64 {
    match app.layout.view(app.focus) {
        Some(slate_core::layout::View::Editor(id)) => *id,
        other => panic!("focused {other:?}"),
    }
}
fn cursor(app: &slate_core::App) -> usize {
    app.views[&focused_view(app)].cursor
}
fn wait(app: &mut slate_core::App, check: impl Fn(&slate_core::App) -> bool) {
    for _ in 0..500 {
        app.process_events();
        if check(app) {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("Timed out: {}", app.status);
}
fn render(app: &mut slate_core::App, width: u16, height: u16) -> String {
    let snapshot = app.snapshot(width, height, 0, 1, 1, 0);
    let pane = snapshot.panes.iter().find(|p| p.kind == "editor").unwrap();
    pane.screen
        .as_ref()
        .unwrap()
        .cells
        .iter()
        .map(|row| {
            row.iter()
                .filter(|c| !c.continuation)
                .map(|c| c.text.as_str())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn multiple_files_new_files_and_positions() {
    let (dir, _serial) = isolated();
    let a = dir.path().join("a.txt");
    fs::write(&a, "1\n2\n3\n").unwrap();
    let new = dir.path().join("new.txt");
    let mut app = launch(&[(&a, Some(3)), (&new, None)]);
    let tabs = app.layout.pane_mut(2).unwrap().0.len();
    assert_eq!(tabs, 2, "Both files open as tabs");
    assert_eq!(cursor(&app), 4, "+3 places the cursor on line 3");
    assert!(app.file_mode && !app.wants_recovery());
    app.dispatch(Command::SwitchTab { pane: 2, index: 1 });
    assert!(!app.dirty(), "A new, empty file is not modified");
    typed(&mut app, "created");
    app.dispatch(Command::Save);
    wait(&mut app, |a| a.status == "Saved");
    assert_eq!(fs::read_to_string(&new).unwrap(), "created");
}

#[test]
fn encodings_and_line_endings_can_be_changed() {
    let (dir, _serial) = isolated();
    let path = dir.path().join("f.txt");
    fs::write(&path, "one\ntwo\n").unwrap();
    let mut app = launch(&[(&path, None)]);
    action(&mut app, "set-line-ending", "crlf");
    assert!(app.dirty(), "Changing the line ending is an unsaved change");
    action(&mut app, "set-encoding", "utf-16le");
    action(&mut app, "toggle-bom", "");
    app.dispatch(Command::Save);
    wait(&mut app, |a| a.status == "Saved");
    let bytes = fs::read(&path).unwrap();
    let expected: Vec<u8> = b"\xFF\xFE"
        .iter()
        .copied()
        .chain("one\r\ntwo\r\n".encode_utf16().flat_map(u16::to_le_bytes))
        .collect();
    assert_eq!(bytes, expected);
    // Reinterpret a Latin-1 file that was opened as UTF-8 by mistake.
    let latin = dir.path().join("l.txt");
    fs::write(&latin, "Ã©".as_bytes()).unwrap();
    let mut app = launch(&[(&latin, None)]);
    action(&mut app, "reopen-encoding", "utf-8");
    wait(&mut app, |a| a.status.starts_with("Reloaded"));
    assert_eq!(text(&app), "Ã©");
    action(&mut app, "reopen-encoding", "latin1");
    wait(&mut app, |a| text(a) != "Ã©");
    assert!(app.status.starts_with("Reloaded"), "{}", app.status);
    // Each UTF-8 byte becomes one windows-1252 character.
    assert_eq!(text(&app), "Ã\u{192}Â©");
    // Opening a legacy file from inside Slate reports its encoding.
    let legacy = dir.path().join("legacy.txt");
    fs::write(&legacy, b"caf\xe9").unwrap();
    app.dispatch(Command::Open { path: legacy });
    wait(&mut app, |a| a.status.starts_with("Opened"));
    assert!(app.status.contains("windows-1252"), "{}", app.status);
    // Characters a legacy encoding cannot store are refused, not mangled.
    action(&mut app, "set-encoding", "latin1");
    typed(&mut app, "✓");
    app.dispatch(Command::Save);
    wait(&mut app, |a| {
        a.status.starts_with("Error") || a.status.starts_with("Save failed")
    });
    assert!(app.status.contains("cannot store"), "{}", app.status);
}

#[test]
fn quit_prompt_saves_everything_or_cancels() {
    let (dir, _serial) = isolated();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    fs::write(&a, "a").unwrap();
    fs::write(&b, "b").unwrap();
    let mut app = launch(&[(&a, None), (&b, None)]);
    typed(&mut app, "1");
    app.dispatch(Command::SwitchTab { pane: 2, index: 1 });
    typed(&mut app, "2");
    app.dispatch(Command::Quit { force: false });
    assert_eq!(app.prompt.as_ref().unwrap().kind, "quit");
    assert!(app.prompt.as_ref().unwrap().input.contains("2 unsaved"));
    key(&mut app, "Escape");
    assert!(app.prompt.is_none() && !app.quit);
    app.dispatch(Command::Quit { force: false });
    typed(&mut app, "y");
    wait(&mut app, |a| a.quit);
    assert_eq!(fs::read_to_string(&a).unwrap(), "1a");
    assert_eq!(fs::read_to_string(&b).unwrap(), "2b");
}

#[test]
fn nano_cut_chains_mark_and_uncut() {
    let (dir, _serial) = isolated();
    let path = dir.path().join("n.txt");
    fs::write(&path, "one\ntwo\nthree\nfour\n").unwrap();
    let mut app = launch(&[(&path, None)]);
    action(&mut app, "cut-line", "");
    action(&mut app, "cut-line", "");
    assert_eq!(
        app.clipboard, "one\ntwo\n",
        "Consecutive cuts collect lines"
    );
    key(&mut app, "Down");
    action(&mut app, "cut-line", "");
    assert_eq!(app.clipboard, "four\n", "Moving starts a new cut");
    action(&mut app, "uncut", "");
    assert_eq!(text(&app), "three\nfour\n");
    // The mark extends selections with plain movement keys.
    ctrl(&mut app, "Home");
    action(&mut app, "mark", "");
    key(&mut app, "Right");
    key(&mut app, "Right");
    action(&mut app, "copy-line", "");
    assert_eq!(app.clipboard, "th");
    assert!(!app.views[&focused_view(&app)].mark, "Copy ends the mark");
}

#[test]
fn justify_and_hard_wrap_use_the_wrap_column() {
    let (dir, _serial) = isolated();
    let path = dir.path().join("p.txt");
    fs::write(&path, "  alpha beta gamma delta epsilon zeta\n\nnext\n").unwrap();
    let mut app = launch(&[(&path, None)]);
    app.dispatch(Command::Configure {
        name: "wrap-column".into(),
        value: "16".into(),
    });
    action(&mut app, "justify", "");
    assert_eq!(
        text(&app),
        "  alpha beta\n  gamma delta\n  epsilon zeta\n\nnext\n"
    );
    app.dispatch(Command::Configure {
        name: "hard-wrap".into(),
        value: "true".into(),
    });
    ctrl(&mut app, "End");
    typed(&mut app, " words that run past");
    assert_eq!(
        text(&app),
        // The continuation keeps the line's indentation.
        "  alpha beta\n  gamma delta\n  epsilon zeta\n\nnext\n words that run\n past"
    );
}

#[test]
fn soft_wrap_renders_rows_and_moves_by_visual_line() {
    let (dir, _serial) = isolated();
    let path = dir.path().join("w.txt");
    fs::write(&path, "aaaa bbbb cccc dddd\nshort\n").unwrap();
    let mut app = launch(&[(&path, None)]);
    app.dispatch(Command::Configure {
        name: "line-numbers".into(),
        value: "false".into(),
    });
    app.dispatch(Command::Configure {
        name: "soft-wrap".into(),
        value: "true".into(),
    });
    let screen = render(&mut app, 12, 6);
    assert_eq!(
        screen.lines().take(4).collect::<Vec<_>>(),
        ["aaaa bbbb", "cccc dddd", "short", ""]
    );
    key(&mut app, "Right");
    key(&mut app, "Down");
    assert_eq!(cursor(&app), 11, "Down stays in the wrapped line");
    key(&mut app, "Down");
    assert_eq!(cursor(&app), 21, "then reaches the next line");
    // Clicking the second visual row lands in the wrapped part of line 1.
    app.dispatch(Command::Click {
        pane: app.focus,
        row: 1,
        col: 2,
        shift: false,
    });
    assert_eq!(cursor(&app), 12);
    // A very long wrapped line keeps the cursor row visible.
    let long = "x".repeat(500);
    fs::write(&path, format!("{long}\n")).unwrap();
    let mut app = launch(&[(&path, None)]);
    app.dispatch(Command::Configure {
        name: "soft-wrap".into(),
        value: "true".into(),
    });
    ctrl(&mut app, "End");
    render(&mut app, 40, 5);
    let view = &app.views[&focused_view(&app)];
    assert!(view.top_row > 0, "The view scrolled within the long line");
}

#[test]
fn gutter_grows_with_line_count() {
    let (dir, _serial) = isolated();
    let path = dir.path().join("many.txt");
    fs::write(&path, "x\n".repeat(123_456)).unwrap();
    let mut app = launch(&[(&path, Some(123_456))]);
    let screen = render(&mut app, 40, 4);
    assert!(
        screen.lines().any(|l| l.starts_with("123456  x")),
        "{screen}"
    );
}

#[test]
fn word_movement_and_deletion() {
    let (dir, _serial) = isolated();
    let path = dir.path().join("w.txt");
    fs::write(&path, "let foo = bar;").unwrap();
    let mut app = launch(&[(&path, None)]);
    ctrl(&mut app, "Right");
    assert_eq!(cursor(&app), 3);
    ctrl(&mut app, "Right");
    assert_eq!(cursor(&app), 7);
    ctrl(&mut app, "Backspace");
    assert_eq!(text(&app), "let  = bar;");
    key(&mut app, "End");
    ctrl(&mut app, "Left");
    assert_eq!(cursor(&app), 7);
}

#[test]
fn insert_file_help_and_statistics() {
    let (dir, _serial) = isolated();
    let path = dir.path().join("main.txt");
    let other = dir.path().join("other.txt");
    fs::write(&path, "end").unwrap();
    fs::write(&other, "inserted\r\n").unwrap();
    let mut app = launch(&[(&path, None)]);
    action(&mut app, "insert-file", other.to_str().unwrap());
    assert_eq!(text(&app), "inserted\nend");
    action(&mut app, "word-count", "");
    assert_eq!(app.status, "Document: 2 lines, 2 words, 12 characters");
    action(&mut app, "location", "");
    assert!(app.status.starts_with("line 2/2"), "{}", app.status);
    action(&mut app, "help", "");
    let help = text(&app);
    assert!(
        help.contains("Ctrl+s") && help.contains("Save document"),
        "{help}"
    );
    assert!(app
        .documents
        .values()
        .any(|d| d.read_only && d.text().contains("keymap")));
}

#[test]
fn single_files_checkpoint_nothing_and_workspaces_store_only_dirty_text() {
    let (dir, _serial) = isolated();
    let workspace = dir.path().join("ws");
    fs::create_dir(&workspace).unwrap();
    let clean = workspace.join("secret.env");
    let dirty = workspace.join("draft.txt");
    fs::write(&clean, "TOKEN=hunter2\n").unwrap();
    fs::write(&dirty, "draft\n").unwrap();
    let mut app = slate_core::App::new(&workspace).unwrap();
    assert!(app.wants_recovery());
    let store =
        slate_core::workspace::WorkspaceStore::acquire_in(&workspace, &dir.path().join("st"))
            .unwrap();
    let session = store.path.clone();
    app.attach_workspace(store, true).unwrap();
    app.dispatch(Command::Open {
        path: clean.clone(),
    });
    wait(&mut app, |a| a.status.starts_with("Opened"));
    app.dispatch(Command::Open {
        path: dirty.clone(),
    });
    wait(&mut app, |a| a.status.starts_with("Opened"));
    typed(&mut app, "more ");
    app.flush_workspace().unwrap();
    let checkpoint = fs::read_to_string(&session).unwrap();
    assert!(
        checkpoint.contains("more draft"),
        "Unsaved text is recoverable"
    );
    assert!(
        !checkpoint.contains("hunter2"),
        "Clean file contents stay out of the checkpoint"
    );
}

#[test]
fn very_long_lines_stay_responsive() {
    let (dir, _serial) = isolated();
    let path = dir.path().join("long.txt");
    // Unoptimised test builds are ~20× slower, so they use a shorter line.
    let megabytes = if cfg!(debug_assertions) { 2 } else { 10 };
    fs::write(&path, "y".repeat(megabytes * 1024 * 1024)).unwrap();
    let mut app = launch(&[(&path, None)]);
    ctrl(&mut app, "End");
    render(&mut app, 120, 40);
    let start = std::time::Instant::now();
    for _ in 0..10 {
        typed(&mut app, "z");
        render(&mut app, 120, 40);
    }
    let elapsed = start.elapsed();
    // The audit measured 1.2 s per keystroke on a 10 MB line in release builds.
    assert!(
        elapsed < Duration::from_secs(if cfg!(debug_assertions) { 10 } else { 2 }),
        "10 keystrokes at the end of a {megabytes} MB line took {elapsed:?}"
    );
}
