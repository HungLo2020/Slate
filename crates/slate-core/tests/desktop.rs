//! Desktop-editor behaviour shared by both frontends: external file changes,
//! recent files, word/line selection, horizontal scrolling, the minimap
//! outline and requests that only a graphical frontend can fulfil.
mod common;
use slate_core::{App, Command};
use std::{fs, path::Path, thread, time::Duration};

use common::isolated;
fn wait(app: &mut App, what: &str, check: impl Fn(&App) -> bool) {
    for _ in 0..600 {
        app.process_events();
        if check(app) {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("Timed out waiting for {what}: {}", app.status);
}
fn editor(app: &App) -> u64 {
    match app.layout.view(app.focus) {
        Some(slate_core::layout::View::Editor(id)) => *id,
        other => panic!("focused {other:?}"),
    }
}
fn text(app: &App) -> String {
    app.documents[&app.views[&editor(app)].document]
        .text()
        .to_string()
}
fn typed(app: &mut App, s: &str) {
    for c in s.chars() {
        app.dispatch(Command::Key {
            key: slate_core::Key {
                key: c.to_string(),
                text: c.to_string(),
                ..Default::default()
            },
        });
    }
}
/// Rewrite a file so its size or modification time visibly changes.
fn rewrite(path: &Path, content: &str) {
    thread::sleep(Duration::from_millis(20));
    fs::write(path, content).unwrap();
}

#[test]
fn clean_documents_follow_their_file_and_modified_ones_ask() {
    let (dir, _serial) = isolated();
    let path = dir.path().join("watched.txt");
    fs::write(&path, "one\n").unwrap();
    let mut app = App::new(&path).unwrap();
    rewrite(&path, "two\n");
    wait(&mut app, "automatic reload", |a| text(a) == "two\n");
    assert!(app.status.contains("reloaded"), "{}", app.status);
    assert!(!app.dirty());

    // Modified buffers are never replaced without asking.
    typed(&mut app, "mine ");
    rewrite(&path, "three\n");
    wait(&mut app, "change prompt", |a| {
        a.prompt.as_ref().is_some_and(|p| p.kind == "file-changed")
    });
    assert_eq!(text(&app), "mine two\n");
    // Keep: the next save deliberately replaces the disk version.
    app.dispatch(Command::Key {
        key: slate_core::Key {
            key: "k".into(),
            text: "k".into(),
            ..Default::default()
        },
    });
    assert!(app.prompt.is_none());
    assert!(app.status.starts_with("Kept your version"));
    app.dispatch(Command::Save);
    wait(&mut app, "save", |a| a.status == "Saved");
    assert_eq!(fs::read_to_string(&path).unwrap(), "mine two\n");

    // Reload: the disk version replaces the buffer.
    typed(&mut app, "x");
    rewrite(&path, "from disk\n");
    wait(&mut app, "second prompt", |a| {
        a.prompt.as_ref().is_some_and(|p| p.kind == "file-changed")
    });
    app.dispatch(Command::SubmitPrompt { all: false });
    assert_eq!(text(&app), "from disk\n");
    assert!(!app.dirty());

    // Touching a file without changing it is not a change.
    let before = app.status.clone();
    filetime_touch(&path);
    for _ in 0..150 {
        app.process_events();
        thread::sleep(Duration::from_millis(10));
    }
    assert!(app.prompt.is_none());
    assert_eq!(app.status, before);

    // Deleting the file marks the buffer so closing or quitting asks.
    fs::remove_file(&path).unwrap();
    wait(&mut app, "deletion", |a| a.status.contains("deleted"));
    assert!(
        app.dirty(),
        "A deleted file's buffer must be saved or discarded"
    );
}

fn filetime_touch(path: &Path) {
    let file = fs::OpenOptions::new().append(true).open(path).unwrap();
    file.set_modified(std::time::SystemTime::now() + Duration::from_secs(5))
        .unwrap();
}

#[test]
fn auto_reload_can_be_turned_off() {
    let (dir, _serial) = isolated();
    let path = dir.path().join("manual.txt");
    fs::write(&path, "a\n").unwrap();
    let mut app = App::new(&path).unwrap();
    app.dispatch(Command::Configure {
        name: "auto-reload".into(),
        value: "false".into(),
    });
    rewrite(&path, "b\n");
    wait(&mut app, "prompt", |a| {
        a.prompt.as_ref().is_some_and(|p| p.kind == "file-changed")
    });
    app.dispatch(Command::DismissPrompt);
    assert_eq!(text(&app), "a\n");
}

#[test]
fn recent_files_are_remembered_across_sessions() {
    let (dir, _serial) = isolated();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    fs::write(&a, "a").unwrap();
    fs::write(&b, "b").unwrap();
    let mut app = App::new(dir.path()).unwrap();
    for path in [&a, &b] {
        app.dispatch(Command::Open { path: path.clone() });
        wait(&mut app, "open", |x| x.status.starts_with("Opened"));
    }
    drop(app);
    let mut app = App::new(dir.path()).unwrap();
    let recent = app.recent_files().to_vec();
    assert_eq!(recent[0], b.to_string_lossy());
    assert_eq!(recent[1], a.to_string_lossy());
    app.dispatch(Command::Action {
        name: "open-recent".into(),
        argument: "2".into(),
    });
    wait(&mut app, "open recent", |x| x.status.starts_with("Opened"));
    assert_eq!(text(&app), "a");
    app.dispatch(Command::Action {
        name: "clear-recent".into(),
        argument: String::new(),
    });
    assert!(app.recent_files().is_empty());
}

#[test]
fn double_and_triple_clicks_select_words_and_lines() {
    let (dir, _serial) = isolated();
    let path = dir.path().join("s.txt");
    fs::write(&path, "alpha beta_gamma delta\nnext\n").unwrap();
    let mut app = App::new(&path).unwrap();
    app.snapshot(80, 10, 0, 1, 1, 0);
    let pane = app.focus;
    let pointer = |kind: &str, col| Command::Pointer {
        pane,
        row: 0,
        col,
        kind: kind.into(),
        button: 0,
        shift: false,
        ctrl: false,
        alt: false,
    };
    app.dispatch(pointer("double", 6 + 8));
    let v = &app.views[&editor(&app)];
    assert_eq!((v.anchor, v.cursor), (Some(6), 16), "beta_gamma");
    app.dispatch(pointer("triple", 6 + 2));
    let v = &app.views[&editor(&app)];
    assert_eq!((v.anchor, v.cursor), (Some(0), 23), "the whole line");
}

#[test]
fn horizontal_scrolling_and_the_minimap_outline() {
    let (dir, _serial) = isolated();
    let path = dir.path().join("wide.txt");
    let mut content = String::new();
    for i in 0..10_000 {
        content.push_str(&format!("{}line {i}\n", "    ".repeat(i % 4)));
    }
    content.push_str(&"w".repeat(300));
    fs::write(&path, &content).unwrap();
    let mut app = App::new(&path).unwrap();
    let pane = app.focus;
    let overview = app.pane_overview(pane).unwrap();
    assert_eq!(overview.total, 10_001);
    assert!(overview.lines.len() <= 4000, "Large documents are sampled");
    assert_eq!(overview.widest, 300);
    // Sample 1 of 4000 over 10 001 lines is document line 2: "        line 2".
    assert_eq!(overview.lines[0], [0, 6]);
    assert_eq!(overview.lines[1], [8, 14]);
    app.snapshot(60, 10, 0, 1, 1, 0);
    app.dispatch(Command::ScrollColumns { pane, delta: 20 });
    app.snapshot(60, 10, 0, 1, 1, 0);
    assert_eq!(
        app.views[&editor(&app)].left,
        20,
        "The cursor does not pull it back"
    );
    // Moving the cursor brings it back into view.
    app.dispatch(Command::Key {
        key: slate_core::Key {
            key: "Right".into(),
            ..Default::default()
        },
    });
    app.snapshot(60, 10, 0, 1, 1, 0);
    // The cursor (column 1) is the first visible column again.
    assert_eq!(app.views[&editor(&app)].left, 1);
}

#[test]
fn graphical_only_actions_become_frontend_requests() {
    let (dir, _serial) = isolated();
    let path = dir.path().join("p.txt");
    fs::write(&path, "print me").unwrap();
    let mut app = App::new(&path).unwrap();
    // The terminal interface cannot print or open windows.
    assert!(!app
        .command_catalog("print")
        .iter()
        .any(|c| c.id == "print" && c.enabled));
    app.terminal_frontend = false;
    for action in ["print", "new-window"] {
        app.dispatch(Command::InvokeAction {
            id: action.into(),
            argument: String::new(),
        });
    }
    assert_eq!(app.take_frontend_requests(), ["print", "new-window"]);
    assert_eq!(
        app.active_document(),
        Some(("p.txt".to_string(), "print me".to_string()))
    );
    for (action, expected) in [("zoom-in", 12), ("zoom-in", 13), ("zoom-reset", 11)] {
        app.dispatch(Command::InvokeAction {
            id: action.into(),
            argument: String::new(),
        });
        assert_eq!(app.preferences.font_size, expected);
    }
}

#[test]
fn terminals_follow_the_editor_theme() {
    let (dir, _serial) = isolated();
    let mut app = App::new_with_startup(
        dir.path(),
        Some(slate_core::preferences::StartupMode::Workspace),
    )
    .unwrap();
    app.dispatch(Command::Configure {
        name: "theme".into(),
        value: "light".into(),
    });
    let snapshot = app.snapshot(160, 40, 1, 1, 1, 3);
    let terminal = snapshot
        .panes
        .iter()
        .find(|p| p.kind == "terminal")
        .unwrap();
    let screen = terminal.screen.as_ref().unwrap();
    let corner = screen.cells.last().unwrap().last().unwrap();
    assert_eq!(
        corner.bg, snapshot.background,
        "Default terminal cells use the theme"
    );
    assert_eq!(snapshot.background, "#ffffff");
}

#[test]
fn files_handed_over_by_another_invocation_open_at_their_line() {
    let (dir, _serial) = isolated();
    let first = dir.path().join("first.txt");
    let second = dir.path().join("second.txt");
    fs::write(&first, "1").unwrap();
    fs::write(&second, "a\nb\nc\n").unwrap();
    let mut app = App::new(&first).unwrap();
    let inbox: slate_core::instance::Inbox = Default::default();
    app.attach_inbox(inbox.clone());
    inbox.lock().unwrap().push(slate_core::cli::LaunchFile {
        path: second.clone(),
        line: Some(3),
        column: None,
    });
    app.events().notify();
    wait(&mut app, "handover", |a| a.status.starts_with("Opened"));
    assert_eq!(text(&app), "a\nb\nc\n");
    assert_eq!(app.views[&editor(&app)].cursor, 4, "Line 3");
    assert_eq!(app.take_frontend_requests(), ["raise"]);
}

#[test]
fn broken_configuration_files_are_reported_and_never_overwritten() {
    let (dir, _serial) = isolated();
    let config = dir.path().join("config/slate");
    fs::create_dir_all(&config).unwrap();
    let settings = "indent_width = \"four\"\n";
    let layouts = "[presets.main\n";
    fs::write(config.join("settings.toml"), settings).unwrap();
    fs::write(config.join("layouts.toml"), layouts).unwrap();
    let path = dir.path().join("a.txt");
    fs::write(&path, "a\n").unwrap();
    let mut app = App::new(&path).unwrap();
    assert!(app.status.contains("rror"), "{}", app.status);
    app.dispatch(Command::Configure {
        name: "soft-wrap".into(),
        value: "true".into(),
    });
    assert!(app.status.contains("settings.toml"), "{}", app.status);
    app.dispatch(Command::SaveLayout {
        name: "mine".into(),
    });
    assert!(app.status.contains("layouts.toml"), "{}", app.status);
    assert_eq!(
        fs::read_to_string(config.join("settings.toml")).unwrap(),
        settings
    );
    assert_eq!(
        fs::read_to_string(config.join("layouts.toml")).unwrap(),
        layouts
    );
}
