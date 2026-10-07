use slate_core::{
    layout::View,
    preferences::{Preferences, StartupMode},
    workspace::WorkspaceStore,
    App, Command, Key,
};
use std::{
    fs,
    time::{Duration, Instant},
};

fn snapshot(app: &mut App) -> slate_core::Snapshot {
    app.snapshot(160, 40, 1, 1, 1, 3)
}

fn key(app: &mut App, name: &str) {
    app.dispatch(Command::Key {
        key: Key {
            key: name.into(),
            ..Default::default()
        },
    });
}

#[test]
fn explicit_startup_modes_work_for_files_and_directories() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("edit.txt");
    fs::write(&file, "original").unwrap();
    for path in [file.as_path(), dir.path()] {
        for mode in [StartupMode::EditorOnly, StartupMode::Workspace] {
            let mut app = App::new_with_startup(path, Some(mode)).unwrap();
            let view = snapshot(&mut app);
            assert_eq!(view.editor_only, mode == StartupMode::EditorOnly);
            assert_eq!(view.panes.len(), if view.editor_only { 1 } else { 3 });
            assert_eq!(app.terminals.len(), if view.editor_only { 0 } else { 1 });
            assert_eq!(app.layout.panes().len(), 3);
            if path == file {
                assert_eq!(app.documents[&10].text(), "original");
            }
        }
    }
}

#[test]
fn collapse_preserves_tabs_dirty_buffers_geometry_and_live_shells() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new_with_startup(dir.path(), Some(StartupMode::Workspace)).unwrap();
    app.dispatch(Command::New);
    app.dispatch(Command::Paste {
        text: "unsaved".into(),
    });
    app.dispatch(Command::ResizeSplit { id: 4, ratio: 0.3 });
    let layout = serde_json::to_string(&app.layout).unwrap();
    let view_count = app.views.len();
    app.terminals
        .get_mut(&12)
        .unwrap()
        .write(b"SLATE_LAYOUT_TEST=preserved\r")
        .unwrap();
    key(&mut app, "F10");
    assert_eq!(snapshot(&mut app).panes.len(), 1);
    // Viewport dimensions may change, but editing state must not.
    let cursor = app
        .views
        .values()
        .find(|v| app.documents[&v.document].dirty())
        .unwrap()
        .cursor;
    assert_eq!(cursor, "unsaved".len());
    key(&mut app, "F6");
    assert_eq!(app.focus, 2);
    key(&mut app, "F10");
    assert_eq!(snapshot(&mut app).panes.len(), 3);
    assert_eq!(serde_json::to_string(&app.layout).unwrap(), layout);
    assert_eq!(app.views.len(), view_count);
    app.terminals
        .get_mut(&12)
        .unwrap()
        .write(b"printf 'SHELL_%s' \"$SLATE_LAYOUT_TEST\"\r")
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !app.terminals[&12]
        .screen()
        .cells
        .iter()
        .flatten()
        .map(|cell| cell.text.as_str())
        .collect::<String>()
        .contains("SHELL_preserved")
    {
        assert!(
            Instant::now() < deadline,
            "Collapsing/reopening replaced the shell"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    app.dispatch(Command::Undo);
    assert!(!app.dirty());
    app.dispatch(Command::Focus { pane: 3 });
    key(&mut app, "F10");
    assert!(matches!(app.layout.view(app.focus), Some(View::Editor(_))));
    assert_eq!(snapshot(&mut app).panes[0].kind, "editor");
}

#[test]
fn editor_only_recovery_defers_shells_and_retains_requested_dirty_file() {
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let file = dir.path().join("edit.txt");
    fs::write(&file, "original").unwrap();
    let layout;
    {
        let mut app = App::new_with_startup(&file, Some(StartupMode::Workspace)).unwrap();
        app.attach_workspace(
            WorkspaceStore::acquire_in(dir.path(), state.path()).unwrap(),
            false,
        )
        .unwrap();
        app.dispatch(Command::Paste {
            text: "unsaved ".into(),
        });
        app.dispatch(Command::ResizeSplit { id: 5, ratio: 0.6 });
        app.dispatch(Command::Focus { pane: 3 });
        layout = serde_json::to_string(&app.layout).unwrap();
    }
    let mut app = App::new_with_startup(&file, Some(StartupMode::EditorOnly)).unwrap();
    app.attach_workspace(
        WorkspaceStore::acquire_in(dir.path(), state.path()).unwrap(),
        true,
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while app.status == "Opening file…" {
        app.process_events();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(app.terminals.is_empty());
    let view = snapshot(&mut app);
    assert!(view.editor_only);
    assert_eq!(view.panes.len(), 1);
    assert_eq!(view.panes[0].kind, "editor");
    assert_eq!(app.documents[&10].text(), "unsaved original");
    assert_eq!(fs::read_to_string(&file).unwrap(), "original");
    key(&mut app, "F10");
    assert_eq!(snapshot(&mut app).panes.len(), 3);
    assert_eq!(serde_json::to_string(&app.layout).unwrap(), layout);
    assert_eq!(app.terminals.len(), 1);
}

#[test]
fn legacy_settings_get_startup_defaults_and_invalid_modes_are_rejected() {
    let settings: Preferences = toml::from_str("indent_width = 2\ntheme = 'light'").unwrap();
    assert_eq!(settings.file_startup, StartupMode::EditorOnly);
    assert_eq!(settings.directory_startup, StartupMode::Workspace);
    assert!(settings
        .global_keys
        .values()
        .any(|v| v == "toggle-workspace"));
    assert!(toml::from_str::<Preferences>("file_startup = 'invalid'").is_err());
    let restored: Preferences = toml::from_str(&toml::to_string(&settings).unwrap()).unwrap();
    assert_eq!(restored.menu_items(), settings.menu_items());
}

#[test]
fn selecting_a_remembered_tool_tab_expands_and_starts_its_deferred_shell() {
    let dir = tempfile::tempdir().unwrap();
    for next_tab in [false, true] {
        let mut app = App::new_with_startup(dir.path(), Some(StartupMode::EditorOnly)).unwrap();
        app.layout.pane_mut(2).unwrap().0.push(View::Terminal(12));
        assert!(app.terminals.is_empty());
        if next_tab {
            app.dispatch(Command::NextTab);
        } else {
            app.dispatch(Command::SwitchTab { pane: 2, index: 1 });
        }
        assert!(!app.editor_only);
        assert_eq!(app.terminals.len(), 1);
        assert_eq!(snapshot(&mut app).panes.len(), 3);
        app.dispatch(Command::EditorOnly);
        assert_eq!(snapshot(&mut app).panes[0].kind, "editor");
    }
}
