//! IDE editing: multiple cursors, bracket pairing, smart indentation,
//! bracket matching and folding, through the same commands both frontends
//! send.
use slate_core::{App, Command, Key};
use std::fs;

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn editor(text: &str) -> (App, tempfile::TempDir, std::sync::MutexGuard<'static, ()>) {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
    std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
    let path = dir.path().join("code.rs");
    fs::write(&path, text).unwrap();
    let mut app = App::new(&path).unwrap();
    app.snapshot(100, 30, 0, 1, 1, 0);
    (app, dir, guard)
}
fn id(app: &App) -> u64 {
    match app.layout.view(app.focus) {
        Some(slate_core::layout::View::Editor(id)) => *id,
        other => panic!("{other:?}"),
    }
}
fn text(app: &App) -> String {
    app.documents[&app.views[&id(app)].document].text()
}
fn press(app: &mut App, key: &str) {
    app.dispatch(Command::Key {
        key: Key {
            key: key.into(),
            ..Default::default()
        },
    });
}
fn chord(app: &mut App, key: &str, ctrl: bool, shift: bool) {
    app.dispatch(Command::Key {
        key: Key {
            key: key.into(),
            ctrl,
            shift,
            ..Default::default()
        },
    });
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
fn action(app: &mut App, name: &str) {
    app.dispatch(Command::Action {
        name: name.into(),
        argument: String::new(),
    });
}
/// The text of one editor row as drawn, without the gutter.
fn row(app: &mut App, y: usize) -> String {
    let frame = app.snapshot(100, 30, 0, 1, 1, 0);
    let pane = frame.panes.iter().find(|p| p.kind == "editor").unwrap();
    pane.screen.as_ref().unwrap().cells[y]
        .iter()
        .map(|c| c.text.as_str())
        .collect::<String>()
}

#[test]
fn several_carets_type_delete_and_undo_together() {
    let (mut app, _dir, _serial) = editor("one\ntwo\nthree\n");
    action(&mut app, "add-cursor-below");
    action(&mut app, "add-cursor-below");
    assert_eq!(app.views[&id(&app)].extra.len(), 2);
    typed(&mut app, "> ");
    assert_eq!(text(&app), "> one\n> two\n> three\n");
    press(&mut app, "End");
    typed(&mut app, ";");
    assert_eq!(text(&app), "> one;\n> two;\n> three;\n");
    press(&mut app, "Backspace");
    assert_eq!(text(&app), "> one\n> two\n> three\n");
    // Copy collects one line per caret; paste hands one back to each.
    press(&mut app, "Home");
    chord(&mut app, "End", false, true);
    app.dispatch(Command::Copy);
    assert_eq!(app.clipboard, "> one\n> two\n> three");
    press(&mut app, "End");
    app.dispatch(Command::Paste {
        text: "a\nb\nc".into(),
    });
    assert_eq!(text(&app), "> onea\n> twob\n> threec\n");
    // Undo restores the whole multi-caret edit at once.
    app.dispatch(Command::Undo);
    assert_eq!(text(&app), "> one\n> two\n> three\n");
    press(&mut app, "Escape");
    assert!(app.views[&id(&app)].extra.is_empty());
    typed(&mut app, "!");
    assert_eq!(text(&app).matches('!').count(), 1);
}

#[test]
fn occurrences_become_carets_and_alt_click_adds_one() {
    let (mut app, _dir, _serial) = editor("let foo = foo + bar(foo);\n");
    app.dispatch(Command::Click {
        pane: app.focus,
        row: 0,
        col: 6 + 5,
        shift: false,
    });
    action(&mut app, "add-next-occurrence");
    let v = &app.views[&id(&app)];
    assert_eq!((v.anchor, v.cursor), (Some(4), 7), "selects the word first");
    action(&mut app, "add-next-occurrence");
    assert_eq!(app.views[&id(&app)].extra.len(), 1);
    typed(&mut app, "x");
    assert_eq!(text(&app), "let x = x + bar(foo);\n");
    app.dispatch(Command::Undo);
    action(&mut app, "select-all-occurrences");
    assert!(app.status.contains("3 occurrences"), "{}", app.status);
    typed(&mut app, "y");
    assert_eq!(text(&app), "let y = y + bar(y);\n");
    press(&mut app, "Escape");
    // Alt+click places another caret.
    let pane = app.focus;
    app.dispatch(Command::Click {
        pane,
        row: 0,
        col: 6,
        shift: false,
    });
    app.dispatch(Command::Pointer {
        pane,
        row: 0,
        col: 6 + 8,
        kind: "press".into(),
        button: 0,
        shift: false,
        ctrl: false,
        alt: true,
    });
    typed(&mut app, "_");
    assert_eq!(text(&app), "_let y = _y + bar(y);\n");
    // Extra carets are drawn.
    assert_eq!(app.views[&id(&app)].extra.len(), 1);
}

#[test]
fn brackets_and_quotes_pair_and_enter_indents() {
    let (mut app, _dir, _serial) = editor("");
    typed(&mut app, "fn f(");
    assert_eq!(text(&app), "fn f()");
    typed(&mut app, ")");
    assert_eq!(text(&app), "fn f()", "typing the closer steps over it");
    typed(&mut app, " {");
    assert_eq!(text(&app), "fn f() {}");
    press(&mut app, "Enter");
    assert_eq!(text(&app), "fn f() {\n    \n}");
    typed(&mut app, "let s = \"");
    assert_eq!(text(&app), "fn f() {\n    let s = \"\"\n}");
    press(&mut app, "Backspace");
    assert_eq!(
        text(&app),
        "fn f() {\n    let s = \n}",
        "empty pair deleted"
    );
    typed(&mut app, "it's");
    assert!(text(&app).contains("it's\n"), "no pair inside a word");
    // A closer typed on an indented blank line lines up with its opener.
    press(&mut app, "End");
    press(&mut app, "Enter");
    press(&mut app, "Enter");
    typed(&mut app, "]");
    assert!(text(&app).ends_with("    ]\n}"), "{:?}", text(&app));
    app.dispatch(Command::Configure {
        name: "auto-close-brackets".into(),
        value: "false".into(),
    });
    chord(&mut app, "End", true, false);
    typed(&mut app, "(");
    assert!(text(&app).ends_with('('), "pairing can be turned off");
}

#[test]
fn selections_wrap_in_brackets_and_matching_brackets_are_found() {
    let (mut app, _dir, _serial) = editor("call(a[1], b)\n");
    chord(&mut app, "Right", true, true);
    typed(&mut app, "[");
    assert_eq!(text(&app), "[call](a[1], b)\n");
    app.dispatch(Command::Undo);
    press(&mut app, "Home");
    for _ in 0..4 {
        press(&mut app, "Right");
    }
    action(&mut app, "go-to-bracket");
    assert_eq!(app.views[&id(&app)].cursor, 12);
    action(&mut app, "go-to-bracket");
    assert_eq!(app.views[&id(&app)].cursor, 4);
    // The pair is highlighted on screen.
    let frame = app.snapshot(100, 30, 0, 1, 1, 0);
    let cells = &frame.panes[0].screen.as_ref().unwrap().cells[0];
    let gutter = cells.iter().position(|c| c.text == "c").unwrap();
    let background = cells[0].bg.clone();
    assert_ne!(cells[gutter + 4].bg, background, "opening bracket");
    assert_ne!(cells[gutter + 12].bg, background, "closing bracket");
    assert_eq!(cells[gutter + 6].bg, background);
}

#[test]
fn folding_hides_indented_regions_and_movement_skips_them() {
    let (mut app, _dir, _serial) =
        editor("fn a() {\n    one\n    two\n}\nfn b() {\n    three\n}\n");
    action(&mut app, "fold");
    assert!(row(&mut app, 0).contains("▸"), "{}", row(&mut app, 0));
    assert!(row(&mut app, 0).contains("⋯"));
    assert!(
        row(&mut app, 1).trim_start().starts_with("4"),
        "{}",
        row(&mut app, 1)
    );
    press(&mut app, "Down");
    let v = &app.views[&id(&app)];
    assert_eq!(
        app.documents[&v.document].line_of(v.cursor),
        3,
        "moving down skips the folded lines"
    );
    press(&mut app, "Up");
    action(&mut app, "unfold");
    assert!(row(&mut app, 1).contains("one"));
    action(&mut app, "fold-all");
    assert!(row(&mut app, 1).contains("}"));
    assert!(row(&mut app, 2).contains("fn b"));
    // Going to a hidden line reveals it.
    app.dispatch(Command::GoToLine { line: 6 });
    app.snapshot(100, 30, 0, 1, 1, 0);
    assert!(
        row(&mut app, 0).contains("▸"),
        "the first region stays folded"
    );
    assert!(row(&mut app, 2).contains("fn b"));
    assert!(row(&mut app, 3).contains("three"));
    // Clicking the marker of a folded header unfolds it.
    action(&mut app, "fold-all");
    let pane = app.focus;
    app.dispatch(Command::Click {
        pane,
        row: 0,
        col: 1,
        shift: false,
    });
    assert!(row(&mut app, 1).contains("one"));
    // Folds follow edits above them.
    app.dispatch(Command::GoToLine { line: 1 });
    press(&mut app, "Home");
    typed(&mut app, "// top\n");
    let header = row(&mut app, 5);
    assert!(header.contains("fn b") && header.contains("▸"), "{header}");
    assert!(
        row(&mut app, 6).trim_start().starts_with("8"),
        "line 7 stays hidden"
    );
}

#[test]
fn snippets_expand_and_tab_visits_their_fields() {
    let (mut app, dir, _serial) = editor("");
    typed(&mut app, "fn");
    press(&mut app, "Tab");
    assert_eq!(text(&app), "fn name() -> () {\n    \n}");
    typed(&mut app, "main");
    press(&mut app, "Tab");
    typed(&mut app, "x: u8");
    press(&mut app, "Tab");
    // The optional return type is selected as a whole; delete it.
    press(&mut app, "Backspace");
    assert_eq!(text(&app), "fn main(x: u8) {\n    \n}");
    press(&mut app, "Tab");
    press(&mut app, "Tab");
    typed(&mut app, "body();");
    assert_eq!(text(&app), "fn main(x: u8) {\n    body();\n}");
    // Tab after the last field is an ordinary Tab again.
    press(&mut app, "Escape");

    // User snippets, with a mirrored field edited in both places.
    let config = dir.path().join("config/slate");
    fs::create_dir_all(&config).unwrap();
    fs::write(
        config.join("snippets.toml"),
        "[[snippet]]\nprefix = \"tag\"\nbody = \"<${1:div}>$0</$1>\"\n",
    )
    .unwrap();
    chord(&mut app, "End", true, false);
    press(&mut app, "Enter");
    typed(&mut app, "tag");
    press(&mut app, "Tab");
    assert!(text(&app).ends_with("<div></div>"), "{:?}", text(&app));
    typed(&mut app, "span");
    assert!(text(&app).ends_with("<span></span>"), "{:?}", text(&app));
    press(&mut app, "Tab");
    typed(&mut app, "hi");
    assert!(text(&app).ends_with("<span>hi</span>"), "{:?}", text(&app));
    // Undo removes the expansion edits step by step back to the prefix.
    app.dispatch(Command::Undo);
    assert!(text(&app).ends_with("<span></span>"));
}
