//! Helpers shared by the integration tests. Every test runs against a
//! private config and state folder, never the developer's own settings,
//! trusted folders or recovery checkpoints.
#![allow(dead_code)]
use slate_core::{layout::View, App};
use std::{
    sync::{Mutex, MutexGuard, OnceLock},
    thread,
    time::Duration,
};

/// Point this test process at a private config and state folder (once).
pub fn isolate() {
    static FOLDER: OnceLock<tempfile::TempDir> = OnceLock::new();
    FOLDER.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
        std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
        dir
    });
}

static SERIAL: Mutex<()> = Mutex::new(());

/// A fresh config and state folder for one test. Tests holding the guard
/// run one at a time, since they change the process environment.
pub fn isolated() -> (tempfile::TempDir, MutexGuard<'static, ()>) {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
    std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
    (dir, guard)
}

/// Process events until `check` holds, or fail naming `what`.
pub fn settle(app: &mut App, what: &str, check: impl Fn(&mut App) -> bool) {
    for _ in 0..1500 {
        app.process_events();
        if check(app) {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("Timed out waiting for {what}: {}", app.status);
}

/// The first document opened (the one a new window starts with).
pub fn first_document(app: &App) -> u64 {
    *app.documents.keys().next().expect("a document")
}

/// The first editor view.
pub fn first_view(app: &App) -> u64 {
    *app.views.keys().next().expect("an editor view")
}

/// The first terminal.
pub fn first_terminal(app: &App) -> u64 {
    *app.terminals.keys().next().expect("a terminal")
}

fn pane_showing(app: &App, kind: impl Fn(&View) -> bool) -> u64 {
    app.layout
        .panes()
        .into_iter()
        .find(|p| app.layout.view(*p).is_some_and(&kind))
        .expect("a pane showing that view")
}

/// The first pane showing an editor.
pub fn editor_pane(app: &App) -> u64 {
    pane_showing(app, |v| matches!(v, View::Editor(_)))
}

/// The first pane showing a terminal.
pub fn terminal_pane(app: &App) -> u64 {
    pane_showing(app, |v| matches!(v, View::Terminal(_)))
}

/// The side pane (files or Git).
pub fn side_pane(app: &App) -> u64 {
    pane_showing(app, |v| matches!(v, View::Files | View::Git))
}
