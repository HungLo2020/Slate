//! Workspace navigation: quick open, symbols, open documents, project search
//! and replace, and going back after jumps.
use slate_core::{App, Command, Key};
use std::{fs, thread, time::Duration};

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn workspace() -> (App, tempfile::TempDir, std::sync::MutexGuard<'static, ()>) {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
    std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
    let root = dir.path().join("project");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("target")).unwrap();
    fs::write(root.join(".gitignore"), "target/\n").unwrap();
    fs::write(
        root.join("src/main.rs"),
        "mod util;\n\nfn main() {\n    util::greet(\"world\");\n}\n",
    )
    .unwrap();
    fs::write(
        root.join("src/util.rs"),
        "pub struct Greeter;\n\npub fn greet(name: &str) {\n    println!(\"hello {name}\");\n}\n",
    )
    .unwrap();
    fs::write(root.join("target/junk.rs"), "fn greet() {}\n").unwrap();
    fs::write(root.join("notes.md"), "# Notes\n\ngreet people\n").unwrap();
    let app = App::new(&root).unwrap();
    (app, dir, guard)
}
fn settle(app: &mut App, what: &str, check: impl Fn(&mut App) -> bool) {
    for _ in 0..500 {
        app.process_events();
        if check(app) {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("Timed out waiting for {what}: {}", app.status);
}
fn typed(app: &mut App, s: &str) {
    for c in s.chars() {
        app.dispatch(Command::Key {
            key: Key {
                key: c.to_string(),
                text: c.to_string(),
                ..Default::default()
            },
        });
    }
}
fn press(app: &mut App, key: &str) {
    app.dispatch(Command::Key {
        key: Key {
            key: key.into(),
            ..Default::default()
        },
    });
}
fn active_text(app: &App) -> Option<(String, usize)> {
    match app.layout.view(app.focus) {
        Some(slate_core::layout::View::Editor(id)) => {
            let v = &app.views[id];
            Some((app.documents[&v.document].title(), v.cursor))
        }
        _ => None,
    }
}
fn picker(app: &mut App) -> slate_core::picker::PickerView {
    app.snapshot(120, 40, 1, 1, 1, 3)
        .picker
        .expect("a picker is open")
}

#[test]
fn quick_open_finds_files_by_fuzzy_name_and_ignores_build_output() {
    let (mut app, _dir, _serial) = workspace();
    app.dispatch(Command::Action {
        name: "quick-open".into(),
        argument: String::new(),
    });
    settle(&mut app, "the index", |a| !picker(a).busy);
    let all: Vec<String> = picker(&mut app)
        .items
        .iter()
        .map(|i| i.label.clone())
        .collect();
    assert!(all.contains(&"src/util.rs".to_string()), "{all:?}");
    assert!(
        !all.iter().any(|p| p.starts_with("target")),
        "ignored files stay out"
    );
    typed(&mut app, "utl");
    let view = picker(&mut app);
    assert_eq!(view.items[0].label, "src/util.rs");
    assert!(!view.items[0].positions.is_empty());
    press(&mut app, "Enter");
    settle(&mut app, "the file", |a| {
        active_text(a).is_some_and(|(title, _)| title == "util.rs")
    });
    assert!(app.snapshot(120, 40, 1, 1, 1, 3).picker.is_none());
}

#[test]
fn symbols_and_open_documents_jump_within_the_workspace() {
    let (mut app, dir, _serial) = workspace();
    let util = dir.path().join("project/src/util.rs");
    app.dispatch(Command::Open { path: util });
    settle(&mut app, "open", |a| a.status.starts_with("Opened"));
    app.dispatch(Command::Action {
        name: "go-to-symbol".into(),
        argument: String::new(),
    });
    settle(&mut app, "symbols", |a| !picker(a).busy);
    let names: Vec<String> = picker(&mut app)
        .items
        .iter()
        .map(|i| i.label.clone())
        .collect();
    assert!(
        names.contains(&"Greeter".into()) && names.contains(&"greet".into()),
        "{names:?}"
    );
    typed(&mut app, "greet");
    // Choose the function, not the struct.
    let index = picker(&mut app)
        .items
        .iter()
        .position(|i| i.label == "greet")
        .unwrap();
    app.dispatch(Command::PickerAccept { index: Some(index) });
    let (title, cursor) = active_text(&app).unwrap();
    assert_eq!(
        (title.as_str(), cursor),
        ("util.rs", 28),
        "line 3, at the name"
    );
    // Back returns to where the jump started.
    app.dispatch(Command::Action {
        name: "go-back".into(),
        argument: String::new(),
    });
    assert_eq!(active_text(&app).unwrap().1, 0);
    app.dispatch(Command::Action {
        name: "go-forward".into(),
        argument: String::new(),
    });
    assert_eq!(active_text(&app).unwrap().1, 28);
    // The open-documents picker switches between buffers. Opening the file
    // replaced the empty Untitled tab.
    app.dispatch(Command::OpenPicker {
        kind: "documents".into(),
        query: String::new(),
    });
    let names: Vec<String> = picker(&mut app)
        .items
        .iter()
        .map(|i| i.label.clone())
        .collect();
    assert_eq!(names, ["util.rs"]);
    press(&mut app, "Escape");
    assert!(app.snapshot(120, 40, 1, 1, 1, 3).picker.is_none());
}

#[test]
fn project_search_streams_results_and_replace_rewrites_files() {
    let (mut app, dir, _serial) = workspace();
    let root = dir.path().join("project");
    // An open, modified document is searched as edited.
    app.dispatch(Command::Open {
        path: root.join("notes.md"),
    });
    settle(&mut app, "open", |a| a.status.starts_with("Opened"));
    app.dispatch(Command::Paste {
        text: "greet first\n".into(),
    });
    app.dispatch(Command::Action {
        name: "search-in-files".into(),
        argument: String::new(),
    });
    typed(&mut app, "greet");
    settle(&mut app, "search", |a| !picker(a).busy);
    let view = picker(&mut app);
    assert_eq!(view.total, 5, "{:?}", view.items);
    assert!(
        view.message.starts_with("5 results in 3 files"),
        "{}",
        view.message
    );
    assert!(view.items.iter().all(|i| !i.detail.starts_with("target")));
    // A case-sensitive search drops "Greeter".
    app.dispatch(Command::PickerOption {
        name: "case".into(),
    });
    settle(&mut app, "case search", |a| !picker(a).busy);
    assert_eq!(picker(&mut app).total, 4);
    // Replace everywhere: the open document changes in the editor, the
    // others on disk.
    app.dispatch(Command::Key {
        key: Key {
            key: "h".into(),
            ctrl: true,
            ..Default::default()
        },
    });
    assert_eq!(app.prompt.as_ref().unwrap().kind, "project-replace");
    app.dispatch(Command::UpdatePrompt {
        input: "welcome".into(),
        replacement: String::new(),
        case_sensitive: false,
        whole_word: false,
    });
    app.dispatch(Command::SubmitPrompt { all: false });
    assert_eq!(app.prompt.as_ref().unwrap().kind, "confirm-replace");
    typed(&mut app, "y");
    settle(&mut app, "replace", |a| a.status.starts_with("Replaced"));
    assert!(
        app.status.starts_with("Replaced 4 matches in 3 files"),
        "{}",
        app.status
    );
    let util = fs::read_to_string(root.join("src/util.rs")).unwrap();
    assert!(
        util.contains("pub fn welcome(") && util.contains("Greeter"),
        "{util}"
    );
    assert!(fs::read_to_string(root.join("src/main.rs"))
        .unwrap()
        .contains("util::welcome("));
    assert_eq!(
        fs::read_to_string(root.join("target/junk.rs")).unwrap(),
        "fn greet() {}\n",
        "ignored files are untouched"
    );
    let notes = app
        .documents
        .values()
        .find(|d| d.path.as_ref().is_some_and(|p| p.ends_with("notes.md")))
        .unwrap();
    assert!(notes.text().starts_with("welcome first\n"));
    assert!(
        notes.dirty(),
        "open documents are left for the user to save"
    );
    assert_eq!(
        fs::read_to_string(root.join("notes.md")).unwrap(),
        "# Notes\n\ngreet people\n"
    );
}

#[test]
fn workbench_keys_never_reach_past_a_terminal() {
    let (mut app, _dir, _serial) = workspace();
    let ctrl_p = Key {
        key: "p".into(),
        ctrl: true,
        ..Default::default()
    };
    let pane_with = |app: &App, kind: &str| {
        app.layout
            .panes()
            .into_iter()
            .find(|p| match app.layout.view(*p) {
                Some(slate_core::layout::View::Terminal(_)) => kind == "terminal",
                Some(slate_core::layout::View::Files) => kind == "files",
                Some(slate_core::layout::View::Editor(_)) => kind == "editor",
                _ => false,
            })
            .unwrap()
    };
    for (kind, expected) in [
        ("terminal", None),
        ("files", Some("quick-open")),
        ("editor", Some("quick-open")),
    ] {
        let pane = pane_with(&app, kind);
        app.dispatch(Command::Focus { pane });
        assert_eq!(app.key_binding(&ctrl_p), expected, "{kind}");
    }
    // Editing keys stay in editors.
    let pane = pane_with(&app, "files");
    app.dispatch(Command::Focus { pane });
    let ctrl_d = Key {
        key: "d".into(),
        ctrl: true,
        ..Default::default()
    };
    assert_eq!(app.key_binding(&ctrl_d), None);
}

#[test]
fn replace_in_files_skips_changed_and_mixed_files_and_can_be_undone() {
    let (mut app, dir, _serial) = workspace();
    let root = dir.path().join("project");
    fs::write(root.join("mixed.txt"), "greet\r\nthere\nok\n").unwrap();
    fs::write(root.join("later.txt"), "greet later\n").unwrap();
    app.dispatch(Command::Action {
        name: "search-in-files".into(),
        argument: String::new(),
    });
    typed(&mut app, "greet");
    settle(&mut app, "search", |a| !picker(a).busy);
    // A file changes after the search.
    fs::write(root.join("later.txt"), "greet changed\n").unwrap();
    app.dispatch(Command::Action {
        name: "replace-in-files".into(),
        argument: String::new(),
    });
    app.dispatch(Command::UpdatePrompt {
        input: "hello".into(),
        replacement: String::new(),
        case_sensitive: false,
        whole_word: false,
    });
    app.dispatch(Command::SubmitPrompt { all: false });
    typed(&mut app, "y");
    settle(&mut app, "replace", |a| a.status.starts_with("Replaced"));
    assert!(
        app.status.contains("later.txt: changed since the search"),
        "{}",
        app.status
    );
    assert!(
        app.status.contains("mixed.txt: mixed line endings"),
        "{}",
        app.status
    );
    assert_eq!(
        fs::read_to_string(root.join("later.txt")).unwrap(),
        "greet changed\n"
    );
    assert_eq!(
        fs::read(root.join("mixed.txt")).unwrap(),
        b"greet\r\nthere\nok\n"
    );
    let util = root.join("src/util.rs");
    assert!(fs::read_to_string(&util).unwrap().contains("fn hello("));
    // Undo restores what it rewrote on disk.
    app.dispatch(Command::Action {
        name: "undo-replace-in-files".into(),
        argument: String::new(),
    });
    assert!(app.status.starts_with("Restored 3 files"), "{}", app.status);
    assert!(fs::read_to_string(&util).unwrap().contains("fn greet("));
}

#[test]
fn going_back_skips_closed_documents() {
    let (mut app, dir, _serial) = workspace();
    let root = dir.path().join("project");
    for name in ["src/main.rs", "src/util.rs"] {
        app.dispatch(Command::Open {
            path: root.join(name),
        });
        settle(&mut app, "open", |a| a.status.starts_with("Opened"));
    }
    // Jump within util.rs, then close main.rs from history's point of view.
    app.dispatch(Command::OpenPicker {
        kind: "documents".into(),
        query: "main".into(),
    });
    app.dispatch(Command::PickerAccept { index: Some(0) });
    app.dispatch(Command::OpenPicker {
        kind: "documents".into(),
        query: "util".into(),
    });
    app.dispatch(Command::PickerAccept { index: Some(0) });
    app.dispatch(Command::CloseDocument { force: true });
    app.dispatch(Command::Action {
        name: "go-back".into(),
        argument: String::new(),
    });
    assert!(
        !app.status.starts_with("Error: That document"),
        "{}",
        app.status
    );
}
