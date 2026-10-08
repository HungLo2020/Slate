//! Tasks: a configured build runs (after trusting the folder), its output
//! streams into a document, and compiler messages become problems shown in
//! the editor.
use slate_core::{App, Command};
use std::{fs, thread, time::Duration};

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn settle(app: &mut App, what: &str, check: impl Fn(&mut App) -> bool) {
    for _ in 0..1500 {
        app.process_events();
        if check(app) {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("Timed out waiting for {what}: {}", app.status);
}

#[test]
fn a_build_task_reports_compiler_problems() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
    std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
    let root = dir.path().join("project");
    fs::create_dir_all(root.join(".slate")).unwrap();
    fs::write(
        root.join("main.c"),
        "int main(void) {\n    int x = 1;\n    return y;\n}\n",
    )
    .unwrap();
    fs::write(
        root.join(".slate/tasks.toml"),
        "[[task]]\nname = \"compile\"\ncommand = \"gcc -Wall -c main.c -o /dev/null\"\ngroup = \"build\"\nmatcher = \"gcc\"\n",
    )
    .unwrap();
    let mut app = App::new(&root).unwrap();
    app.dispatch(Command::Open {
        path: root.join("main.c"),
    });
    settle(&mut app, "open", |a| a.status.starts_with("Opened"));

    // Restricted mode asks first.
    app.dispatch(Command::Action {
        name: "run-build-task".into(),
        argument: String::new(),
    });
    assert_eq!(app.prompt.as_ref().unwrap().kind, "trust");
    app.dispatch(Command::SubmitPrompt { all: false });
    settle(&mut app, "the task", |a| a.status.starts_with("compile "));
    assert!(
        app.status
            .starts_with("compile failed (exit 1) · 1 error, 1 warning"),
        "{}",
        app.status
    );
    // Output went to a task document.
    let output = app
        .documents
        .values()
        .find(|d| d.title().starts_with("Task: compile"))
        .expect("task output document");
    assert!(output.text().contains("undeclared"), "{}", output.text());
    assert!(output.text().ends_with("[compile failed (exit 1)]\n"));
    assert_eq!(output.title(), "Task: compile · failed (exit 1)");
    // Problems point into the source.
    let frame = app.snapshot(120, 40, 1, 1, 1, 3);
    assert_eq!(frame.problems, (1, 1));
    app.dispatch(Command::OpenPicker {
        kind: "diagnostics".into(),
        query: String::new(),
    });
    let picker = app.snapshot(120, 40, 1, 1, 1, 3).picker.unwrap();
    assert!(
        picker.items[0].label.contains("undeclared"),
        "{:?}",
        picker.items
    );
    assert!(
        picker.items[0].detail.contains("main.c:3:12"),
        "{}",
        picker.items[0].detail
    );
    app.dispatch(Command::PickerAccept { index: Some(0) });
    let view = match app.layout.view(app.focus) {
        Some(slate_core::layout::View::Editor(id)) => app.views[id].clone(),
        other => panic!("{other:?}"),
    };
    let doc = &app.documents[&view.document];
    assert_eq!(doc.line_col(view.cursor, 4), (2, 11));

    // Fixing the code and rebuilding clears the error.
    fs::write(root.join("main.c"), "int main(void) {\n    return 0;\n}\n").unwrap();
    app.dispatch(Command::Action {
        name: "run-task".into(),
        argument: "compile".into(),
    });
    settle(&mut app, "the rebuild", |a| {
        a.status.starts_with("compile succeeded")
    });
    assert_eq!(app.snapshot(120, 40, 1, 1, 1, 3).problems, (0, 0));

    // A long task can be stopped.
    fs::write(
        root.join(".slate/tasks.toml"),
        "[[task]]\nname = \"wait\"\ncommand = \"sleep 30\"\n",
    )
    .unwrap();
    app.dispatch(Command::Action {
        name: "run-task".into(),
        argument: "wait".into(),
    });
    assert!(app.status.starts_with("Running wait"));
    app.dispatch(Command::Action {
        name: "stop-task".into(),
        argument: String::new(),
    });
    settle(&mut app, "stop", |a| a.status.starts_with("wait "));
    assert!(
        app.status.starts_with("wait failed") || app.status.starts_with("wait stopped"),
        "{}",
        app.status
    );
}

#[test]
fn tasks_stop_when_slate_exits_and_output_reuses_its_tab() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
    std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
    let root = dir.path().join("project");
    fs::create_dir_all(root.join(".slate")).unwrap();
    let marker = dir.path().join("still-running");
    fs::write(
        root.join(".slate/tasks.toml"),
        format!(
            "[[task]]\nname = \"serve\"\ncommand = \"sleep 1; touch '{}'\"\n\n[[task]]\nname = \"hello\"\ncommand = \"printf 'h\\\\303\\\\251llo\\\\n'\"\n",
            marker.display()
        ),
    )
    .unwrap();
    let mut app = App::new(&root).unwrap();
    app.set_session_trust(true);
    // Output arrives whole (no split UTF-8) and a second run reuses the tab.
    for _ in 0..2 {
        app.dispatch(Command::Action {
            name: "run-task".into(),
            argument: "hello".into(),
        });
        settle(&mut app, "hello", |a| a.status.starts_with("hello "));
    }
    let outputs: Vec<_> = app
        .documents
        .values()
        .filter(|d| d.title().starts_with("Task: hello"))
        .collect();
    assert_eq!(outputs.len(), 1, "one output document per task");
    assert!(outputs[0].text().contains("héllo"), "{}", outputs[0].text());
    // A running task is stopped when the editor goes away.
    app.dispatch(Command::Action {
        name: "run-task".into(),
        argument: "serve".into(),
    });
    assert!(app.tasks_running());
    drop(app);
    thread::sleep(Duration::from_millis(1500));
    assert!(!marker.exists(), "the task outlived Slate");
}
