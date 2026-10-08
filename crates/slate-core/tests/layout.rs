//! Pane and layout changes never lose documents or shells, and the limits
//! that saved layouts must satisfy hold while editing.
mod common;
use slate_core::{
    layout::{Axis, View, MAX_TABS},
    preferences::StartupMode,
    App, Command,
};
use std::{collections::BTreeSet, fs};

use common::isolated;
fn workspace(dir: &std::path::Path) -> App {
    App::new_with_startup(dir, Some(StartupMode::Workspace)).unwrap()
}
fn open(app: &mut App, dir: &std::path::Path, name: &str) {
    let path = dir.join(name);
    fs::write(&path, name).unwrap();
    let before = app.documents.len();
    app.dispatch(Command::Open { path });
    for _ in 0..500 {
        app.process_events();
        if app.documents.len() > before || app.status.starts_with("Opened") {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("Timed out opening {name}: {}", app.status);
}
/// Documents shown by some tab in the layout.
fn shown_documents(app: &App) -> BTreeSet<u64> {
    app.layout
        .views()
        .iter()
        .filter_map(|v| match v {
            View::Editor(id) => Some(app.views[id].document),
            _ => None,
        })
        .collect()
}
fn shown_terminals(app: &App) -> BTreeSet<u64> {
    app.layout
        .views()
        .iter()
        .filter_map(|v| match v {
            View::Terminal(id) => Some(*id),
            _ => None,
        })
        .collect()
}
/// Every document and shell is reachable and every view is shown.
fn assert_nothing_leaked(app: &App) {
    let documents: BTreeSet<u64> = app.documents.keys().copied().collect();
    assert_eq!(shown_documents(app), documents, "every document has a tab");
    let shown: BTreeSet<u64> = app
        .layout
        .views()
        .iter()
        .filter_map(|v| match v {
            View::Editor(id) => Some(*id),
            _ => None,
        })
        .collect();
    let views: BTreeSet<u64> = app.views.keys().copied().collect();
    assert_eq!(shown, views, "no hidden editor views");
    let terminals: BTreeSet<u64> = app.terminals.keys().copied().collect();
    assert_eq!(shown_terminals(app), terminals, "every shell has a tab");
    assert!(app.layout.validate(), "{:?}", app.layout);
}

#[test]
fn closing_a_pane_moves_its_tabs_instead_of_hiding_them() {
    let (dir, _serial) = isolated();
    let mut app = workspace(dir.path());
    app.dispatch(Command::Split {
        axis: Axis::Vertical,
        kind: Some("editor".into()),
    });
    open(&mut app, dir.path(), "a.txt");
    open(&mut app, dir.path(), "b.txt");
    app.dispatch(Command::Paste {
        text: "edit ".into(),
    });
    app.dispatch(Command::NewTerminal);
    let before = app.documents.len();
    let shells = app.terminals.len();
    app.dispatch(Command::ClosePane);
    assert!(
        app.status.starts_with("Closed pane · moved"),
        "{}",
        app.status
    );
    assert_eq!(app.documents.len(), before);
    assert_eq!(app.terminals.len(), shells);
    assert!(app.dirty(), "The unsaved edit is still reachable");
    assert_nothing_leaked(&app);
}

#[test]
fn presets_and_saved_layouts_keep_open_documents_and_shells() {
    let (dir, _serial) = isolated();
    let mut app = workspace(dir.path());
    for name in ["a.txt", "b.txt", "c.txt"] {
        open(&mut app, dir.path(), name);
    }
    app.dispatch(Command::NewTerminal);
    app.dispatch(Command::SaveLayout {
        name: "mine".into(),
    });
    let shells: BTreeSet<u64> = app.terminals.keys().copied().collect();
    for preset in ["bottom_terminal", "development"] {
        app.dispatch(Command::Preset {
            name: preset.into(),
        });
        assert_nothing_leaked(&app);
        assert_eq!(
            app.terminals.keys().copied().collect::<BTreeSet<_>>(),
            shells,
            "{preset} neither drops nor starts shells"
        );
    }
    app.dispatch(Command::LoadLayout {
        name: "mine".into(),
    });
    assert_nothing_leaked(&app);
    assert_eq!(
        app.terminals.keys().copied().collect::<BTreeSet<_>>(),
        shells,
        "Loading a layout rebinds the running shells"
    );
    let titles: BTreeSet<String> = app.documents.values().map(|d| d.title()).collect();
    for name in ["a.txt", "b.txt", "c.txt"] {
        assert!(titles.contains(name), "{titles:?}");
    }
}

#[test]
fn tab_and_nesting_limits_hold_while_editing() {
    let (dir, _serial) = isolated();
    let mut app = workspace(dir.path());
    for i in 0..MAX_TABS + 8 {
        open(&mut app, dir.path(), &format!("f{i}.txt"));
    }
    assert!(app.documents.len() >= MAX_TABS + 8);
    for pane in app.layout.panes() {
        assert!(app.layout.tabs(pane).unwrap().len() <= MAX_TABS);
    }
    assert_nothing_leaked(&app);

    let mut refused = false;
    for _ in 0..20 {
        app.dispatch(Command::Split {
            axis: Axis::Horizontal,
            kind: Some("files".into()),
        });
        refused |= app.status.contains("too deeply");
    }
    assert!(refused, "Splitting stops at the depth limit");
    assert!(app.layout.validate());
    // A layout at the limits still round-trips through recovery.
    let saved = serde_json::to_string(&app.layout).unwrap();
    let parsed: slate_core::layout::Node = serde_json::from_str(&saved).unwrap();
    assert!(parsed.validate());
}
