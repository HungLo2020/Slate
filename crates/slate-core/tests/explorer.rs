mod common;

use slate_core::{preferences::StartupMode, workspace::WorkspaceStore, App, Command, Key};
use std::{fs, path::Path};

fn app(root: &Path, terminal: bool) -> App {
    common::isolate();
    let mut app = App::new_with_startup(root, Some(StartupMode::Workspace)).unwrap();
    app.terminal_frontend = terminal;
    app.refresh();
    app
}

fn frame(app: &mut App) -> slate_core::Snapshot {
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
fn both_file_panes_stop_parent_navigation_at_the_workspace_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("workspace");
    let child = root.join("child");
    fs::create_dir_all(&child).unwrap();
    fs::write(child.join("nested.txt"), "nested").unwrap();
    for terminal in [true, false] {
        let mut app = app(&root, terminal);
        common::settle(&mut app, "workspace listing", |a| {
            frame(a).files.iter().any(|e| e.name == "child")
        });
        assert!(!frame(&mut app).files.iter().any(|e| e.name == ".."));
        app.dispatch(Command::Focus {
            pane: common::side_pane(&app),
        });
        if terminal {
            // Exercise the actual TUI folder keys, including repeated Left.
            for _ in 0..3 {
                key(&mut app, "Left");
            }
            assert_eq!(frame(&mut app).browser, root.to_string_lossy());
            key(&mut app, "Right");
        } else {
            app.dispatch(Command::Browse {
                path: child.clone(),
            });
        }
        common::settle(&mut app, "child listing", |a| {
            frame(a).files.iter().any(|e| e.name == "nested.txt")
        });
        assert_eq!(frame(&mut app).files[0].name, "..");
        key(&mut app, "Home");
        key(&mut app, "Enter");
        common::settle(&mut app, "parent listing", |a| {
            frame(a).browser == root.to_string_lossy()
                && frame(a).files.iter().any(|e| e.name == "child")
        });
        assert!(!frame(&mut app).files.iter().any(|e| e.name == ".."));
        assert_eq!(app.root, root);
    }
}

#[test]
fn both_file_panes_reject_outside_directories_but_allow_explicit_file_opening() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("workspace");
    let sibling = dir.path().join("workspace-other");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&sibling).unwrap();
    let outside_file = sibling.join("outside.txt");
    fs::write(&outside_file, "outside text").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&sibling, root.join("outside-link")).unwrap();
    for terminal in [true, false] {
        let mut app = app(&root, terminal);
        let mut paths = vec!["..".into(), sibling.clone()];
        #[cfg(unix)]
        paths.push(root.join("outside-link"));
        for path in paths {
            app.dispatch(Command::Browse { path: path.clone() });
            assert!(
                app.status.contains("outside the workspace"),
                "{}",
                app.status
            );
            assert_eq!(frame(&mut app).browser, root.to_string_lossy());
            app.dispatch(Command::Open { path });
            common::settle(&mut app, "directory rejection", |a| {
                a.status.contains("outside the workspace")
            });
            assert_eq!(frame(&mut app).browser, root.to_string_lossy());
        }
        if !terminal {
            app.dispatch(Command::Focus {
                pane: common::side_pane(&app),
            });
            for id in ["expand-folder", "toggle-folder", "collapse-folder"] {
                app.dispatch(Command::Action {
                    name: id.into(),
                    argument: sibling.to_string_lossy().into_owned(),
                });
                assert!(
                    app.status.contains("outside the workspace"),
                    "{id}: {}",
                    app.status
                );
            }
        }
        app.dispatch(Command::Open {
            path: outside_file.clone(),
        });
        common::settle(&mut app, "explicit outside file", |a| {
            a.documents
                .values()
                .any(|d| d.path.as_ref() == Some(&outside_file))
        });
        assert_eq!(frame(&mut app).browser, root.to_string_lossy());
    }
}

#[test]
fn recovery_keeps_valid_browser_locations_and_resets_outside_ones() {
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let root = dir.path().join("workspace");
    let child = root.join("child");
    fs::create_dir_all(&child).unwrap();
    let mut initial = app(&root, true);
    let store = WorkspaceStore::acquire_in(&root, state.path()).unwrap();
    let checkpoint = store.path.clone();
    initial.attach_workspace(store, false).unwrap();
    initial.dispatch(Command::Paste {
        text: "unsaved".into(),
    });
    initial.flush_workspace().unwrap();
    drop(initial);
    let saved: serde_json::Value = serde_json::from_slice(&fs::read(&checkpoint).unwrap()).unwrap();
    for (browser, expected) in [
        (child.clone(), child.clone()),
        (dir.path().to_path_buf(), root.clone()),
        (root.join("missing"), root.clone()),
    ] {
        let mut old = saved.clone();
        old["browser"] = serde_json::json!(browser);
        fs::write(&checkpoint, serde_json::to_vec(&old).unwrap()).unwrap();
        for terminal in [true, false] {
            let mut restored = app(&root, terminal);
            restored
                .attach_workspace(
                    WorkspaceStore::acquire_in(&root, state.path()).unwrap(),
                    true,
                )
                .unwrap();
            assert_eq!(frame(&mut restored).browser, expected.to_string_lossy());
            assert!(restored
                .documents
                .values()
                .any(|d| d.text() == "unsaved" && d.dirty()));
        }
    }
}

#[test]
fn both_file_panes_collapse_all_folders_and_open_terminals_in_entries() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("workspace");
    let child = root.join("child");
    let outside = dir.path().join("outside");
    fs::create_dir_all(&child).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(child.join("nested.txt"), "nested").unwrap();
    for terminal in [true, false] {
        let mut app = app(&root, terminal);
        common::settle(&mut app, "workspace listing", |a| {
            frame(a).files.iter().any(|e| e.name == "child")
        });
        app.dispatch(Command::Focus {
            pane: common::side_pane(&app),
        });
        if terminal {
            app.dispatch(Command::Browse {
                path: child.clone(),
            });
        } else {
            app.dispatch(Command::Action {
                name: "expand-folder".into(),
                argument: child.to_string_lossy().into_owned(),
            });
        }
        common::settle(&mut app, "child rows", |a| {
            frame(a).files.iter().any(|e| e.name == "nested.txt")
        });
        app.dispatch(Command::Action {
            name: "collapse-all-folders".into(),
            argument: String::new(),
        });
        common::settle(&mut app, "collapsed listing", |a| {
            let f = frame(a);
            f.browser == root.to_string_lossy()
                && f.files.iter().any(|e| e.name == "child" && !e.expanded)
                && !f.files.iter().any(|e| e.name == "nested.txt")
        });

        let before = app.terminals.len();
        app.dispatch(Command::Action {
            name: "terminal-here".into(),
            argument: outside.to_string_lossy().into_owned(),
        });
        assert!(
            app.status.contains("outside the workspace"),
            "{}",
            app.status
        );
        assert_eq!(app.terminals.len(), before);
        // A file starts the terminal in its folder.
        app.dispatch(Command::Action {
            name: "terminal-here".into(),
            argument: child.join("nested.txt").to_string_lossy().into_owned(),
        });
        assert_eq!(app.terminals.len(), before + 1, "{}", app.status);
    }
}
