//! Debugging a real C program through GDB's Debug Adapter Protocol mode:
//! breakpoints, stepping, the call stack, locals, evaluation and output.
use slate_core::{App, Command};
use std::{fs, path::Path, process, thread, time::Duration};

const SOURCE: &str = r#"#include <stdio.h>

int square(int v) {
    int r = v * v;
    return r;
}

int main(void) {
    int x = 3;
    int y = square(x);
    printf("result %d\n", y);
    return y == 9 ? 0 : 1;
}
"#;

fn have(program: &str) -> bool {
    process::Command::new(program)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn settle(app: &mut App, what: &str, check: impl Fn(&mut App) -> bool) {
    for _ in 0..3000 {
        app.process_events();
        if check(app) {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("Timed out waiting for {what}: {}", app.status);
}

fn panel(app: &App) -> String {
    app.documents
        .values()
        .find(|d| d.title().starts_with("Debug: "))
        .map(|d| d.text())
        .unwrap_or_default()
}

fn caret_line(app: &App) -> usize {
    match app.layout.view(app.focus) {
        Some(slate_core::layout::View::Editor(id)) => {
            let v = &app.views[id];
            app.documents[&v.document].line_of(v.cursor) + 1
        }
        _ => 0,
    }
}

/// The focused editor's drawn row for a one-based line.
fn drawn_line(app: &mut App, line: usize) -> String {
    let frame = app.snapshot(120, 40, 1, 1, 1, 3);
    let editor = frame
        .panes
        .iter()
        .find(|p| p.kind == "editor" && p.focused)
        .unwrap();
    let top = editor.editor.as_ref().unwrap().top;
    editor.screen.as_ref().unwrap().cells[line - 1 - top]
        .iter()
        .map(|c| c.text.as_str())
        .collect()
}

fn action(app: &mut App, name: &str, argument: &str) {
    app.dispatch(Command::Action {
        name: name.into(),
        argument: argument.into(),
    });
}

#[test]
fn gdb_debugs_a_c_program() {
    if !have("gdb") || !have("gcc") {
        eprintln!("skipping: gdb and gcc are required");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
    std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
    let root = dir.path().join("project");
    fs::create_dir_all(root.join(".slate")).unwrap();
    let source = root.join("main.c");
    fs::write(&source, SOURCE).unwrap();
    let built = process::Command::new("gcc")
        .args(["-g", "-O0", "-o", "prog", "main.c"])
        .current_dir(&root)
        .status()
        .unwrap();
    assert!(built.success());
    fs::write(
        root.join(".slate/launch.toml"),
        "[[launch]]\nname = \"prog\"\nprogram = \"prog\"\n",
    )
    .unwrap();

    let mut app = App::new(&root).unwrap();
    app.set_session_trust(true);
    app.dispatch(Command::Open {
        path: source.clone(),
    });
    settle(&mut app, "open", |a| a.status.starts_with("Opened"));
    // A breakpoint on `int y = square(x);` (line 10).
    app.dispatch(Command::GoToLine { line: 10 });
    action(&mut app, "toggle-breakpoint", "");
    assert_eq!(app.status, "Set breakpoint at line 10");
    let row = drawn_line(&mut app, 10);
    assert!(row.contains('◆'), "{row}");

    action(&mut app, "debug-start", "");
    settle(&mut app, "the breakpoint", |a| {
        a.status.starts_with("Paused") && panel(a).contains("Locals")
    });
    assert_eq!(caret_line(&app), 10);
    let text = panel(&app);
    assert!(text.contains("Paused (breakpoint)"), "{text}");
    assert!(text.contains("#0 main main.c:10"), "{text}");
    assert!(text.contains("x = 3"), "{text}");
    // The paused line is marked.
    let row = drawn_line(&mut app, 10);
    assert!(row.contains('▶'), "{row}");

    // F11 steps into square; the stack shows both frames.
    app.dispatch(Command::Key {
        key: slate_core::Key {
            key: "F11".into(),
            ..Default::default()
        },
    });
    settle(&mut app, "step into", |a| panel(a).contains("#0 square"));
    assert_eq!(caret_line(&app), 4);
    assert!(panel(&app).contains("#1 main main.c:10"));
    action(&mut app, "debug-step-over", "");
    settle(&mut app, "step over", |a| {
        caret_line(a) == 5 && panel(a).contains("r = 9")
    });
    action(&mut app, "debug-evaluate", "r + v");
    settle(&mut app, "evaluate", |a| a.status == "r + v = 12");
    // Choosing the caller's frame shows its locals and evaluates there.
    action(&mut app, "debug-call-stack", "");
    let frames = app.snapshot(120, 40, 1, 1, 1, 3).picker.unwrap();
    assert!(
        frames.items[1].label.starts_with("#1 main"),
        "{:?}",
        frames.items
    );
    app.dispatch(Command::PickerAccept { index: Some(1) });
    settle(&mut app, "caller locals", |a| {
        panel(a).contains("Locals of #1") && panel(a).contains("x = 3")
    });
    assert_eq!(caret_line(&app), 10);
    action(&mut app, "debug-evaluate", "x * 2");
    settle(&mut app, "evaluate in main", |a| a.status == "x * 2 = 6");
    action(&mut app, "debug-step-out", "");
    settle(&mut app, "step out", |a| panel(a).contains("#0 main"));

    // Continue to the end: output and exit code are reported.
    action(&mut app, "debug-continue", "");
    settle(&mut app, "exit", |a| !a.debugging());
    assert_eq!(app.status, "Program exited with code 0");
    // Output can trail the exit by a moment.
    settle(&mut app, "output", |a| panel(a).contains("result 9"));
    let text = panel(&app);
    assert!(text.contains("result 9"), "{text}");
    assert!(text.contains("Exited with code 0"), "{text}");
    assert!(Path::new(&source).exists());
}

#[test]
fn breakpoints_follow_edits_and_survive_a_restart() {
    use slate_core::workspace::WorkspaceStore;
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
    let root = dir.path().join("project");
    fs::create_dir_all(&root).unwrap();
    let source = root.join("main.c");
    fs::write(&source, SOURCE).unwrap();
    let mut app = App::new(&root).unwrap();
    app.attach_workspace(
        WorkspaceStore::acquire_in(&root, state.path()).unwrap(),
        false,
    )
    .unwrap();
    app.dispatch(Command::Open {
        path: source.clone(),
    });
    settle(&mut app, "open", |a| a.status.starts_with("Opened"));
    app.dispatch(Command::GoToLine { line: 10 });
    action(&mut app, "toggle-breakpoint", "");
    assert!(
        drawn_line(&mut app, 10).contains('◆'),
        "{}",
        drawn_line(&mut app, 10)
    );
    // Two lines inserted above move it to line 12.
    app.dispatch(Command::GoToLine { line: 1 });
    app.dispatch(Command::Paste {
        text: "// one\n// two\n".into(),
    });
    assert!(
        drawn_line(&mut app, 12).contains('◆'),
        "{}",
        drawn_line(&mut app, 12)
    );
    assert!(!drawn_line(&mut app, 10).contains('◆'));
    action(&mut app, "breakpoints", "");
    let list = app.snapshot(120, 40, 1, 1, 1, 3).picker.unwrap();
    assert_eq!(list.items[0].label, "main.c:12");
    app.dispatch(Command::PickerClose);
    app.dispatch(Command::Save);
    settle(&mut app, "save", |a| a.status == "Saved");
    app.flush_workspace().unwrap();
    drop(app);
    // A new session restores it.
    let mut app = App::new(&root).unwrap();
    app.attach_workspace(
        WorkspaceStore::acquire_in(&root, state.path()).unwrap(),
        true,
    )
    .unwrap();
    app.dispatch(Command::Open { path: source });
    settle(&mut app, "open", |a| a.status.starts_with("Opened"));
    app.dispatch(Command::GoToLine { line: 12 });
    assert!(
        drawn_line(&mut app, 12).contains('◆'),
        "{}",
        drawn_line(&mut app, 12)
    );
    action(&mut app, "clear-breakpoints", "");
    assert!(!drawn_line(&mut app, 12).contains('◆'));
}
