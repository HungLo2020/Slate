//! External tools from tools.toml: selection filters, document reports,
//! inserted output, keys, and trust for project tools.
use slate_core::{App, Command, Key};
use std::{fs, thread, time::Duration};

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
fn text(app: &App) -> String {
    match app.layout.view(app.focus) {
        Some(slate_core::layout::View::Editor(id)) => app.documents[&app.views[id].document].text(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn tools_filter_text_report_and_need_trust_for_project_ones() {
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
    std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
    fs::create_dir_all(dir.path().join("config/slate")).unwrap();
    fs::write(
        dir.path().join("config/slate/tools.toml"),
        r#"
[[tool]]
name = "Upper"
command = "tr a-z A-Z"
key = "Alt+u"

[[tool]]
name = "Count lines"
command = "wc -l"
input = "document"
output = "status"

[[tool]]
name = "Where"
command = 'printf "%s:%s" "$(basename "$SLATE_FILE")" "$SLATE_LINE"'
input = "none"
output = "insert"
"#,
    )
    .unwrap();
    let root = dir.path().join("project");
    fs::create_dir_all(root.join(".slate")).unwrap();
    fs::write(
        root.join(".slate/tools.toml"),
        "[[tool]]\nname = \"Shout\"\ncommand = \"sed s/$/!/\"\ninput = \"document\"\nkey = \"Ctrl+s\"\n",
    )
    .unwrap();
    let file = root.join("notes.txt");
    fs::write(&file, "one\ntwo\nthree\n").unwrap();
    let mut app = App::new(&file).unwrap();

    // Listed in the palette.
    let names: Vec<String> = app
        .command_catalog("tool")
        .into_iter()
        .map(|c| c.name)
        .collect();
    assert!(
        names.contains(&"Tool: Upper".into()) && names.contains(&"Tool: Shout".into()),
        "{names:?}"
    );

    // The key runs Upper on the selection (the first line).
    app.dispatch(Command::Key {
        key: Key {
            key: "End".into(),
            shift: true,
            ..Default::default()
        },
    });
    app.dispatch(Command::Key {
        key: Key {
            key: "u".into(),
            alt: true,
            ..Default::default()
        },
    });
    settle(&mut app, "upper", |a| a.status == "Upper done");
    assert_eq!(text(&app), "ONE\ntwo\nthree\n");
    // Undo restores it like any edit.
    app.dispatch(Command::Undo);
    assert_eq!(text(&app), "one\ntwo\nthree\n");

    app.command_line("tool:count-lines");
    settle(&mut app, "count", |a| a.status.starts_with("Count lines:"));
    assert_eq!(app.status, "Count lines: 3");

    app.dispatch(Command::GoToLine { line: 2 });
    app.command_line("tool:where");
    settle(&mut app, "where", |a| a.status == "Where done");
    assert_eq!(text(&app), "one\nnotes.txt:2two\nthree\n");

    // A project cannot bind keys: Ctrl+S still saves.
    assert_eq!(
        app.key_binding(&Key {
            key: "s".into(),
            ctrl: true,
            ..Default::default()
        }),
        Some("save")
    );
    // A project tool asks for trust first.
    app.command_line("tool:shout");
    assert_eq!(app.prompt.as_ref().unwrap().kind, "trust");
    app.dispatch(Command::SubmitPrompt { all: false });
    settle(&mut app, "shout", |a| a.status == "Shout done");
    assert_eq!(text(&app), "one!\nnotes.txt:2two!\nthree!\n");

    // A failing tool reports its error and changes nothing.
    fs::write(
        dir.path().join("config/slate/tools.toml"),
        "[[tool]]\nname = \"Broken\"\ncommand = \"echo nope >&2; exit 2\"\n",
    )
    .unwrap();
    app.dispatch(Command::ReloadSettings);
    app.command_line("tool:broken");
    settle(&mut app, "broken", |a| {
        a.status.starts_with("Broken failed")
    });
    assert_eq!(app.status, "Broken failed: sh: nope");
    assert_eq!(text(&app), "one!\nnotes.txt:2two!\nthree!\n");
}
