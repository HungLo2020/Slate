//! Opening a folder: every route agrees, this window's own folder is shown
//! rather than reopened, a folder open in another window is not opened twice,
//! and the GUI opens others in this window or a new one as chosen.
mod common;
use common::isolated;
use slate_core::{preferences::StartupMode, workspace::WorkspaceStore, App, Command};
use std::fs;

fn open(root: &std::path::Path, terminal: bool) -> App {
    let mut app = App::new_with_startup(root, Some(StartupMode::Workspace)).unwrap();
    app.terminal_frontend = terminal;
    let store = WorkspaceStore::acquire(&app.root).unwrap();
    app.attach_workspace(store, false).unwrap();
    app
}
fn configure(app: &mut App, name: &str, value: &str) {
    app.dispatch(Command::Configure {
        name: name.into(),
        value: value.into(),
    });
    assert!(app.status.starts_with("Set"), "{}", app.status);
}
fn open_folder(app: &mut App, path: &std::path::Path) {
    app.dispatch(Command::OpenWorkspace {
        path: path.to_path_buf(),
    });
}

#[test]
fn the_gui_asks_where_to_open_a_folder_unless_told() {
    let (dir, _serial) = isolated();
    let here = dir.path().join("here");
    let other = dir.path().join("other");
    fs::create_dir_all(&here).unwrap();
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("inside.txt"), "x").unwrap();
    let other = other.canonicalize().unwrap();
    let mut app = open(&here, false);
    let _ = app.take_frontend_requests();

    // This window's own folder is shown, not opened again.
    open_folder(&mut app, &here);
    assert!(app.prompt.is_none() && app.take_frontend_requests().is_empty());
    assert!(
        app.status.contains("already open in this window"),
        "{}",
        app.status
    );

    // By default the GUI asks; cancelling leaves the window alone for good.
    open_folder(&mut app, &other);
    assert_eq!(app.prompt.as_ref().unwrap().kind, "open-folder-where");
    app.dispatch(Command::DismissPrompt);
    for _ in 0..5 {
        app.process_events();
    }
    assert_eq!(app.root, here.canonicalize().unwrap());

    // "New window" asks the frontend for one.
    configure(&mut app, "open-folder-in", "new");
    open_folder(&mut app, &other);
    assert!(app.prompt.is_none());
    assert_eq!(
        app.take_frontend_requests(),
        [format!("open-workspace:{}", other.display())]
    );

    // "This window" replaces the session in place.
    configure(&mut app, "open-folder-in", "current");
    let session = app.session();
    open_folder(&mut app, &other);
    assert_eq!(app.root, other);
    assert_eq!(app.session(), session + 1);
    assert!(!app.terminal_frontend, "the new session keeps its frontend");
}

#[test]
fn a_folder_open_in_another_window_is_not_opened_twice() {
    let (dir, _serial) = isolated();
    let here = dir.path().join("here");
    let other = dir.path().join("other");
    fs::create_dir_all(&here).unwrap();
    fs::create_dir_all(&other).unwrap();
    // Another window holds the other folder's workspace.
    let elsewhere = WorkspaceStore::acquire(&other.canonicalize().unwrap()).unwrap();
    for terminal in [false, true] {
        let mut app = open(&here, terminal);
        let _ = app.take_frontend_requests();
        open_folder(&mut app, &other);
        assert!(
            app.status.contains("already open in another Slate window"),
            "{}",
            app.status
        );
        assert!(app.prompt.is_none() && app.take_frontend_requests().is_empty());
        assert_eq!(app.root, here.canonicalize().unwrap());
    }
    drop(elsewhere);
    // Once that window closes, the folder opens normally.
    let mut app = open(&here, true);
    open_folder(&mut app, &other);
    assert_eq!(app.root, other.canonicalize().unwrap());
}
