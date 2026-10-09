//! Integrated terminals: exited shells close their tabs, programs can set the
//! tab title and copy with OSC 52, and scrollback follows the setting.
use slate_core::{layout::View, preferences::StartupMode, App, Command};
use std::{thread, time::Duration};

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn isolated() -> (tempfile::TempDir, std::sync::MutexGuard<'static, ()>) {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
    std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
    // A predictable shell without bracketed paste or prompt integration.
    std::env::set_var("SHELL", "/bin/sh");
    (dir, guard)
}
fn wait(app: &mut App, what: &str, check: impl Fn(&mut App) -> bool) {
    for _ in 0..1000 {
        app.process_events();
        if check(app) {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("Timed out waiting for {what}: {}", app.status);
}
fn focused_terminal(app: &App) -> u64 {
    match app.layout.view(app.focus) {
        Some(View::Terminal(id)) => *id,
        other => panic!("focused {other:?}"),
    }
}
fn run(app: &mut App, line: &str) {
    app.dispatch(Command::Paste {
        text: format!("{line}\r"),
    });
}
fn tab_titles(app: &mut App) -> Vec<String> {
    app.snapshot(160, 40, 1, 1, 1, 3)
        .panes
        .iter()
        .flat_map(|p| p.tabs.iter().map(|t| t.title.clone()))
        .collect()
}

#[test]
fn programs_set_titles_and_copy_and_exited_shells_close() {
    let (dir, _serial) = isolated();
    let mut app = App::new_with_startup(dir.path(), Some(StartupMode::Workspace)).unwrap();
    app.dispatch(Command::NewTerminal);
    let id = focused_terminal(&app);
    // Programs may not set the clipboard unless the user allows it.
    run(
        &mut app,
        r"printf '\033]2;my build\007\033]52;c;aGVsbG8=\007'",
    );
    wait(&mut app, "title and refused copy", |a| {
        a.status.contains("tried to copy") && tab_titles(a).contains(&"my build".to_string())
    });
    assert!(app.clipboard.is_empty());
    app.dispatch(Command::Configure {
        name: "terminal-clipboard".into(),
        value: "true".into(),
    });
    run(&mut app, r"printf '\033]52;c;aGVsbG8=\007'");
    wait(&mut app, "clipboard", |a| a.clipboard == "hello");

    run(&mut app, "exit 3");
    wait(&mut app, "the shell to be reaped", |a| {
        !a.terminals.contains_key(&id)
    });
    assert!(app.status.contains("exited with code 3"), "{}", app.status);
    assert!(!app.layout.views().contains(&View::Terminal(id)));
    assert!(app.layout.validate());
}

#[test]
fn scrollback_follows_the_setting() {
    let (dir, _serial) = isolated();
    let mut app = App::new_with_startup(dir.path(), Some(StartupMode::Workspace)).unwrap();
    app.dispatch(Command::Configure {
        name: "terminal-scrollback".into(),
        value: "10".into(),
    });
    app.dispatch(Command::NewTerminal);
    let id = focused_terminal(&app);
    run(&mut app, "seq 1 300; echo done-$((40+2))");
    wait(&mut app, "output", |a| {
        a.terminals[&id].screen().cells.iter().any(|row| {
            row.iter()
                .map(|c| c.text.as_str())
                .collect::<String>()
                .contains("done-42")
        })
    });
    assert_eq!(app.terminals[&id].scrollback_len(), 10);
    app.dispatch(Command::Configure {
        name: "terminal-scrollback".into(),
        value: "100001".into(),
    });
    assert!(app.status.contains("terminal_scrollback"), "{}", app.status);
}

#[test]
fn a_shell_closes_even_when_a_background_job_keeps_the_terminal_open() {
    let (dir, _serial) = isolated();
    let mut app = App::new_with_startup(dir.path(), Some(StartupMode::Workspace)).unwrap();
    app.dispatch(Command::NewTerminal);
    let id = focused_terminal(&app);
    run(&mut app, "sleep 30 & exit 0");
    wait(&mut app, "the shell to be reaped", |a| {
        !a.terminals.contains_key(&id)
    });
}

#[test]
fn stalled_pty_rejects_pastes_without_blocking_editor_or_shutdown() {
    use slate_core::terminal::TerminalSession;
    use std::time::Instant;
    let (dir, _serial) = isolated();
    let mut app = App::new_with_startup(dir.path(), Some(StartupMode::Workspace)).unwrap();
    app.dispatch(Command::NewTerminal);
    let id = focused_terminal(&app);
    let terminal = TerminalSession::spawn(
        dir.path(),
        Some("stty -echo -icanon; printf '\\033[?2004hREADY'; exec sleep 30"),
        24,
        80,
    )
    .unwrap();
    app.terminals.insert(id, terminal);
    wait(&mut app, "a stalled terminal", |a| {
        a.terminals[&id]
            .screen()
            .cells
            .iter()
            .flatten()
            .map(|c| c.text.as_str())
            .collect::<String>()
            .contains("READY")
    });
    let paste = "x".repeat(4 * 1024 * 1024 - 12);
    app.dispatch(Command::Paste {
        text: paste.clone(),
    });
    let started = Instant::now();
    app.dispatch(Command::Paste { text: paste });
    assert!(app.status.contains("input is full"), "{}", app.status);
    assert!(started.elapsed() < Duration::from_secs(1));
    let (pane, index) = app
        .layout
        .panes()
        .into_iter()
        .find_map(|pane| {
            let (tabs, _) = app.layout.pane_mut(pane)?;
            tabs.iter()
                .position(|tab| matches!(tab, View::Editor(_)))
                .map(|index| (pane, index))
        })
        .unwrap();
    app.dispatch(Command::SwitchTab { pane, index });
    app.dispatch(Command::Paste {
        text: "editor still works".into(),
    });
    assert!(app
        .documents
        .values()
        .any(|d| d.text() == "editor still works"));
    let started = Instant::now();
    drop(app);
    assert!(started.elapsed() < Duration::from_secs(5));
}
