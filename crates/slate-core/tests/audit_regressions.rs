use slate_core::{
    cli::{Launch, LaunchFile},
    Command, Key,
};
use std::{
    fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn setup() -> (tempfile::TempDir, std::sync::MutexGuard<'static, ()>) {
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
    std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
    (dir, guard)
}
fn open(path: &Path) -> slate_core::App {
    slate_core::App::launch(
        &Launch {
            files: vec![LaunchFile {
                path: path.into(),
                line: None,
                column: None,
            }],
            recover: false,
            ..Default::default()
        },
        None,
    )
    .unwrap()
}
fn type_text(app: &mut slate_core::App, text: &str) {
    app.dispatch(Command::Key {
        key: Key {
            text: text.into(),
            key: text.into(),
            ..Default::default()
        },
    });
}
fn wait(app: &mut slate_core::App, ready: impl Fn(&slate_core::App) -> bool) {
    let until = Instant::now() + Duration::from_secs(5);
    while !ready(app) {
        app.process_events();
        assert!(Instant::now() < until, "{}", app.status);
        thread::sleep(Duration::from_millis(2));
    }
}
#[test]
fn reload_never_replaces_edits_typed_after_request() {
    let (dir, _guard) = setup();
    let path = dir.path().join("test.txt");
    fs::write(&path, "original").unwrap();
    let mut app = open(&path);
    app.command_line("reload");
    type_text(&mut app, "new");
    wait(&mut app, |a| a.status.starts_with("Reload cancelled"));
    assert_eq!(
        app.documents
            .values()
            .find(|d| d.path.as_ref() == Some(&path))
            .unwrap()
            .text(),
        "neworiginal"
    );
}
#[test]
fn save_all_resumes_after_each_untitled_buffer() {
    let (dir, _guard) = setup();
    let path = dir.path().join("named.txt");
    fs::write(&path, "original").unwrap();
    let mut app = open(&path);
    type_text(&mut app, "edited");
    app.command_line("new");
    type_text(&mut app, "first");
    app.command_line("new");
    type_text(&mut app, "second");
    app.command_line("save-all");
    for index in 0..2 {
        wait(&mut app, |a| {
            a.prompt.as_ref().is_some_and(|p| p.kind == "save-as")
        });
        app.prompt.as_mut().unwrap().input = dir
            .path()
            .join(format!("untitled-{index}.txt"))
            .to_string_lossy()
            .into_owned();
        app.dispatch(Command::SubmitPrompt { all: false });
    }
    wait(&mut app, |a| !a.dirty());
    assert_eq!(fs::read_to_string(path).unwrap(), "editedoriginal");
    assert_eq!(
        fs::read_to_string(dir.path().join("untitled-0.txt")).unwrap(),
        "first"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("untitled-1.txt")).unwrap(),
        "second"
    );
}
#[test]
fn save_all_formats_each_named_document_using_its_settings() {
    let (dir, _guard) = setup();
    let config = dir.path().join("config/slate");
    fs::create_dir_all(&config).unwrap();
    fs::write(config.join("languages.toml"), "[[language]]\nname = 'Fake'\nextensions = ['fk']\nformatter = ['python3', '-c', 'import sys; print(sys.stdin.read().upper(), end=\"\")']\n").unwrap();
    fs::write(
        config.join("trusted-folders"),
        dir.path().to_string_lossy().as_bytes(),
    )
    .unwrap();
    let first = dir.path().join("first.fk");
    let second = dir.path().join("second.fk");
    fs::write(&first, "original").unwrap();
    fs::write(&second, "original").unwrap();
    let mut app = open(&first);
    app.command_line("set format-on-save true");
    type_text(&mut app, "one");
    app.command_line(&format!("open {}", second.display()));
    wait(&mut app, |a| a.documents.len() == 2);
    type_text(&mut app, "two");
    app.command_line("save-all");
    wait(&mut app, |a| !a.dirty());
    assert_eq!(fs::read_to_string(first).unwrap(), "ONEORIGINAL");
    assert_eq!(fs::read_to_string(second).unwrap(), "TWOORIGINAL");
}
#[test]
fn crlf_formatter_output_keeps_the_buffer_lf_and_saves_single_crlf() {
    let (dir, _guard) = setup();
    let config = dir.path().join("config/slate");
    fs::create_dir_all(&config).unwrap();
    fs::write(config.join("languages.toml"), "[[language]]\nname = 'Fake'\nextensions = ['fk']\nformatter = ['python3', '-c', 'import sys; sys.stdout.buffer.write(sys.stdin.read().upper().replace(\"\\\\n\", \"\\\\r\\\\n\").encode())']\n").unwrap();
    fs::write(
        config.join("trusted-folders"),
        dir.path().to_string_lossy().as_bytes(),
    )
    .unwrap();
    let path = dir.path().join("one.fk");
    fs::write(&path, "a\r\nb\r\n").unwrap();
    let mut app = open(&path);
    app.command_line("format-document");
    wait(&mut app, |a| a.status == "Formatted");
    let doc = app.documents.values().next().unwrap();
    assert_eq!(doc.text(), "A\nB\n");
    app.command_line("save");
    wait(&mut app, |a| !a.dirty());
    assert_eq!(fs::read(&path).unwrap(), b"A\r\nB\r\n");
}
#[test]
fn close_modified_tab_can_save_then_close() {
    let (dir, _guard) = setup();
    let path = dir.path().join("test.txt");
    fs::write(&path, "original").unwrap();
    let mut app = open(&path);
    type_text(&mut app, "new");
    app.command_line("close");
    assert_eq!(app.prompt.as_ref().unwrap().kind, "close-tab");
    app.dispatch(Command::SubmitPrompt { all: false });
    wait(&mut app, |a| {
        !a.documents.values().any(|d| d.path.as_ref() == Some(&path))
    });
    assert_eq!(fs::read_to_string(path).unwrap(), "neworiginal");
}
#[test]
fn workspace_language_overrides_follow_the_active_document_without_polluting_global_config() {
    let (dir, _guard) = setup();
    fs::create_dir_all(dir.path().join(".slate")).unwrap();
    fs::write(
        dir.path().join(".slate/settings.toml"),
        "[editor]\nindent_width = 3\n[language.rust]\nindent_width = 2\n",
    )
    .unwrap();
    let rust = dir.path().join("one.rs");
    let text = dir.path().join("two.txt");
    fs::write(&rust, "").unwrap();
    fs::write(&text, "").unwrap();
    let mut app = open(&rust);
    assert_eq!(app.preferences.indent_width, 2);
    app.command_line(&format!("open {}", text.display()));
    wait(&mut app, |a| a.documents.len() == 2);
    app.process_events();
    assert_eq!(app.preferences.indent_width, 3);
    app.command_line("set font-size 13");
    assert_eq!(
        slate_core::preferences::Preferences::load()
            .unwrap()
            .indent_width,
        4
    );
}
#[test]
fn zero_tab_width_overrides_follow_the_indent_width() {
    let (dir, _guard) = setup();
    fs::create_dir_all(dir.path().join(".slate")).unwrap();
    fs::write(
        dir.path().join(".slate/settings.toml"),
        "[editor]\nindent_width = 3\ntab_width = 0\n[language.rust]\ntab_width = 0\n",
    )
    .unwrap();
    let rust = dir.path().join("one.rs");
    fs::write(&rust, "\tfn main() {}\n\t\tx\n").unwrap();
    let mut app = open(&rust);
    assert_eq!(app.preferences.tab_width, 3);
    // Rendering, wrapping and moving across tabs never divide by zero.
    app.command_line("set soft-wrap true");
    app.snapshot(40, 10, 1, 1, 1, 3);
    app.dispatch(Command::Key {
        key: Key {
            key: "Down".into(),
            ..Default::default()
        },
    });
    app.snapshot(40, 10, 1, 1, 1, 3);
}
#[test]
fn profiles_restore_preferences_and_layout_after_restart() {
    let (dir, _guard) = setup();
    let path = dir.path().join("test.txt");
    fs::write(&path, "").unwrap();
    let mut app = open(&path);
    app.command_line("set indent-width 2");
    app.command_line("editor-only");
    app.command_line("profile-save coding");
    assert!(app.status.starts_with("Saved profile"), "{}", app.status);
    app.command_line("set indent-width 8");
    app.command_line("profile-load coding");
    assert!(app.status.starts_with("Loaded profile"), "{}", app.status);
    assert_eq!(app.preferences.indent_width, 2);
    assert!(app.editor_only);
    drop(app);
    let restarted = open(&path);
    assert_eq!(restarted.preferences.indent_width, 2);
    assert!(restarted.editor_only);
    assert!(slate_core::profiles::validate_name("../escape").is_err());
}
#[test]
fn invalid_override_does_not_persist_a_new_profile_selection() {
    let (dir, _guard) = setup();
    let path = dir.path().join("test.txt");
    fs::write(&path, "").unwrap();
    let mut app = open(&path);
    let editor_only = app.editor_only;
    app.command_line("set font-size 13");
    fs::create_dir_all(dir.path().join(".slate")).unwrap();
    fs::write(
        dir.path().join(".slate/settings.toml"),
        "[editor]\nindent_width = 0\n",
    )
    .unwrap();
    app.command_line("profile-load minimal");
    assert!(app.status.starts_with("Error:"), "{}", app.status);
    assert_eq!(
        slate_core::preferences::Preferences::load()
            .unwrap()
            .profile,
        "default"
    );
    assert_eq!(app.editor_only, editor_only);
}
#[test]
fn cancelling_native_save_all_dialog_keeps_documents_dirty_and_stops_queue() {
    let (dir, _guard) = setup();
    let path = dir.path().join("test.txt");
    fs::write(&path, "").unwrap();
    let mut app = open(&path);
    app.command_line("new");
    type_text(&mut app, "first");
    app.command_line("new");
    type_text(&mut app, "second");
    app.command_line("save-all");
    app.dispatch(Command::PathDialogStart);
    app.dispatch(Command::PathDialogFinish {
        paths: Vec::new(),
        overwrite: false,
    });
    app.process_events();
    assert!(app.prompt.is_none());
    assert_eq!(app.documents.values().filter(|d| d.dirty()).count(), 2);
    app.command_line("save-all");
    assert_eq!(app.prompt.as_ref().unwrap().kind, "save-as");
}
#[test]
fn native_save_as_remains_bound_to_its_document_after_focus_changes() {
    let (dir, _guard) = setup();
    let path = dir.path().join("test.txt");
    fs::write(&path, "").unwrap();
    let mut app = open(&path);
    app.command_line("new");
    type_text(&mut app, "first");
    app.command_line("save-as");
    app.dispatch(Command::PathDialogStart);
    app.command_line("new");
    type_text(&mut app, "second");
    let saved = dir.path().join("saved.txt");
    app.dispatch(Command::PathDialogFinish {
        paths: vec![saved.clone()],
        overwrite: false,
    });
    wait(&mut app, |_| saved.exists());
    assert_eq!(fs::read_to_string(saved).unwrap(), "first");
    assert!(app
        .documents
        .values()
        .any(|d| d.text() == "second" && d.dirty()));
}
#[cfg(unix)]
#[test]
fn opening_a_named_pipe_is_rejected_without_blocking() {
    let (dir, _guard) = setup();
    let path = dir.path().join("pipe");
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    unsafe {
        assert_eq!(libc::mkfifo(c.as_ptr(), 0o600), 0);
    }
    assert!(slate_core::document::Document::open(&path)
        .err()
        .unwrap()
        .to_string()
        .contains("regular file"));
    let mut app = slate_core::App::new(dir.path()).unwrap();
    type_text(&mut app, "safe content");
    app.dispatch(Command::SaveAs {
        path,
        overwrite: true,
    });
    wait(&mut app, |a| a.status.starts_with("Save failed"));
    assert!(app.dirty());
}
#[test]
fn discard_actions_from_every_input_route_ask_first() {
    let (dir, _guard) = setup();
    let path = dir.path().join("keep.txt");
    fs::write(&path, "saved").unwrap();
    let mut app = open(&path);
    type_text(&mut app, "edit ");
    let key = |app: &mut slate_core::App, name: &str| {
        app.dispatch(Command::Key {
            key: Key {
                key: name.into(),
                ..Default::default()
            },
        })
    };
    let prompt = |app: &slate_core::App| app.prompt.as_ref().map(|p| p.kind.clone());
    // The palette, a typed command and a key binding all ask.
    app.dispatch(Command::InvokeAction {
        id: "discard-quit".into(),
        argument: String::new(),
    });
    assert_eq!(prompt(&app).as_deref(), Some("quit"));
    key(&mut app, "Escape");
    app.command_line("discard-quit");
    assert_eq!(prompt(&app).as_deref(), Some("quit"));
    key(&mut app, "Escape");
    app.preferences
        .global_keys
        .insert("f9".into(), "discard-document".into());
    key(&mut app, "F9");
    assert_eq!(prompt(&app).as_deref(), Some("close-tab"));
    key(&mut app, "Enter");
    assert!(!app.quit);
    let doc = app.documents.values().find(|d| d.path.is_some()).unwrap();
    assert_eq!(doc.text(), "edit saved");
    assert!(doc.dirty());
    // Accepting the prompt discards.
    app.command_line("discard-document");
    key(&mut app, "d");
    assert!(app.documents.values().all(|d| d.path.is_none()));
    // With nothing to lose, discard-and-quit just quits.
    app.dispatch(Command::InvokeAction {
        id: "discard-quit".into(),
        argument: String::new(),
    });
    assert!(app.quit);
    assert_eq!(fs::read_to_string(&path).unwrap(), "saved");
}
