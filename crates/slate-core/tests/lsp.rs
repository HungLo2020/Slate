//! The language-server client against a scripted server
//! (`fixtures/fake_lsp.py`): incremental sync with UTF-16 positions,
//! diagnostics, hover, navigation, rename, formatting, code actions,
//! completion and symbols, and restricted mode.
use slate_core::{App, Command, Key};
use std::{fs, path::PathBuf, thread, time::Duration};

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Fixture {
    app: App,
    dir: tempfile::TempDir,
    file: PathBuf,
    _serial: std::sync::MutexGuard<'static, ()>,
}
fn fixture(text: &str, trusted: bool) -> Fixture {
    let serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config/slate");
    fs::create_dir_all(&config).unwrap();
    let server = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_lsp.py");
    fs::write(
        config.join("languages.toml"),
        format!(
            "[[language]]\nname = \"Fake\"\nextensions = [\"fk\"]\nserver = [\"python3\", \"{}\"]\nsettings = {{ fake = {{ enabled = true }} }}\ninitialization-options = {{ fixture = true }}\n",
            server.display()
        ),
    )
    .unwrap();
    std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
    std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
    let dump = dir.path().join("dump");
    fs::create_dir_all(&dump).unwrap();
    std::env::set_var("FAKE_LSP_DUMP", &dump);
    let project = dir.path().join("project");
    fs::create_dir_all(&project).unwrap();
    let extra = project.join("other.fk");
    fs::write(&extra, "greet from a closed file\n").unwrap();
    std::env::set_var("FAKE_LSP_EXTRA", &extra);
    let file = project.join("main.fk");
    fs::write(&file, text).unwrap();
    let mut app = App::new(&file).unwrap();
    app.set_session_trust(trusted);
    app.snapshot(120, 40, 0, 1, 1, 0);
    Fixture {
        app,
        dir,
        file,
        _serial: serial,
    }
}
impl Fixture {
    fn wait(&mut self, what: &str, check: impl Fn(&mut App) -> bool) {
        for _ in 0..800 {
            self.app.process_events();
            if check(&mut self.app) {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("Timed out waiting for {what}: {}", self.app.status);
    }
    fn editor(&self) -> u64 {
        match self.app.layout.view(self.app.focus) {
            Some(slate_core::layout::View::Editor(id)) => *id,
            other => panic!("{other:?}"),
        }
    }
    fn text(&self) -> String {
        self.app.documents[&self.app.views[&self.editor()].document].text()
    }
    fn cursor(&self) -> usize {
        self.app.views[&self.editor()].cursor
    }
    fn server_text(&self) -> Option<String> {
        fs::read_to_string(self.dir.path().join("dump/main.fk")).ok()
    }
    /// Wait until the server's copy of the document matches the editor's.
    /// Wait until the server's diagnostics for the text have arrived, so
    /// requests that depend on them (code actions) see them.
    fn diagnosed(&mut self) {
        self.wait("diagnostics", |a| {
            let (errors, warnings) = a.snapshot(120, 40, 0, 1, 1, 0).problems;
            errors + warnings > 0
        });
    }
    fn synced(&mut self) {
        let dump = self.dir.path().join("dump/main.fk");
        let doc = self.app.views[&self.editor()].document;
        for _ in 0..800 {
            self.app.process_events();
            let ours = self.app.documents[&doc].text();
            if fs::read_to_string(&dump).ok().as_deref() == Some(ours.as_str()) {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "server text {:?} != editor text {:?}",
            self.server_text(),
            self.app.documents[&doc].text()
        );
    }
    fn typed(&mut self, s: &str) {
        for c in s.chars() {
            self.app.dispatch(Command::Key {
                key: Key {
                    key: c.to_string(),
                    text: c.to_string(),
                    ..Default::default()
                },
            });
        }
    }
    fn press(&mut self, key: &str) {
        self.app.dispatch(Command::Key {
            key: Key {
                key: key.into(),
                ..Default::default()
            },
        });
    }
    fn action(&mut self, name: &str, argument: &str) {
        self.app.dispatch(Command::Action {
            name: name.into(),
            argument: argument.into(),
        });
    }
    fn goto(&mut self, line: usize, column: usize) {
        self.app.dispatch(Command::GoToLine { line });
        let doc = self.app.views[&self.editor()].document;
        let start = self.app.documents[&doc].line_offset(line - 1);
        let id = self.editor();
        self.app.views.get_mut(&id).unwrap().cursor = start + column;
    }
}

const SOURCE: &str = "def greet\ndef main\nmain calls greet 👩‍💻 ERROR\n";

#[test]
fn documents_sync_incrementally_with_utf16_positions() {
    let mut f = fixture(SOURCE, true);
    f.synced();
    // Edits after an astral character exercise UTF-16 columns.
    f.goto(3, "main calls greet 👩‍💻 ".len());
    f.typed("wow ");
    f.synced();
    f.press("Backspace");
    f.synced();
    // Several carets edit as one change; undo reverses it.
    f.goto(1, 0);
    f.action("add-cursor-below", "");
    f.action("add-cursor-below", "");
    f.typed("x");
    f.synced();
    f.app.dispatch(Command::Undo);
    f.synced();
    assert_eq!(f.text(), SOURCE.replace("ERROR", "wowERROR"));
    // Replacing the whole text falls back to a full update.
    f.app.dispatch(Command::SelectAll);
    f.app.dispatch(Command::Paste {
        text: "def fresh\n".into(),
    });
    f.synced();
    // Saving is reported.
    f.app.dispatch(Command::Save);
    let file = f.file.clone();
    f.wait("save", |a| {
        !a.dirty() && fs::read_to_string(&file).is_ok_and(|text| text == "def fresh\n")
    });
    let saved_log = f.dir.path().join("dump/saved");
    f.wait("didSave", |_| saved_log.exists());
}

#[test]
fn diagnostics_hover_and_navigation() {
    let mut f = fixture(SOURCE, true);
    f.wait("diagnostics", |a| a.problem_counts() == (1, 0));
    f.goto(3, "main calls greet 👩‍💻 E".len());
    let frame = f.app.snapshot(120, 40, 0, 1, 1, 0);
    assert_eq!(frame.problems, (1, 0));
    assert_eq!(frame.problem, "error: an error here");
    // Underlined in the error colour, with a gutter mark.
    let screen = frame.panes[0].screen.as_ref().unwrap();
    let row: String = screen.cells[2].iter().map(|c| c.text.as_str()).collect();
    assert!(row.contains('●'), "{row}");
    let e = screen.cells[2]
        .iter()
        .position(|c| c.text == "E" && c.underline)
        .expect("ERROR is underlined");
    assert_ne!(screen.cells[2][e].fg, screen.cells[2][0].fg);
    f.goto(1, 0);
    f.action("next-problem", "");
    assert_eq!(
        f.cursor(),
        "def greet\ndef main\nmain calls greet 👩‍💻 ".len()
    );
    // Problems list.
    f.action("problems", "");
    let picker = f.app.snapshot(120, 40, 0, 1, 1, 0).picker.unwrap();
    assert_eq!(picker.items[0].label, "an error here");
    f.app.dispatch(Command::PickerClose);

    // Hover shows the server's text and closes on Escape.
    f.goto(3, 13);
    f.action("hover", "");
    f.wait("hover", |a| a.snapshot(120, 40, 0, 1, 1, 0).hover.is_some());
    let hover = f.app.snapshot(120, 40, 0, 1, 1, 0).hover.unwrap();
    assert_eq!(hover.text, "greet\nhover for greet");
    f.press("Escape");
    assert!(f.app.snapshot(120, 40, 0, 1, 1, 0).hover.is_none());

    // Definition jumps to `def greet`; back returns.
    f.goto(3, 13);
    let before = f.cursor();
    f.action("go-to-definition", "");
    f.wait("definition", |a| {
        a.status != "x" && {
            let v = a.layout.view(a.focus).cloned();
            matches!(v, Some(slate_core::layout::View::Editor(id)) if a.views[&id].cursor == 4)
        }
    });
    f.action("go-back", "");
    assert_eq!(f.cursor(), before);
    // References open a picker of both uses.
    f.action("find-references", "");
    f.wait("references", |a| {
        a.snapshot(120, 40, 0, 1, 1, 0)
            .picker
            .is_some_and(|p| p.title == "References" && p.total == 2)
    });
    f.app.dispatch(Command::PickerClose);
    // Document symbols come from the server.
    f.action("go-to-symbol", "");
    f.wait("symbols", |a| {
        a.snapshot(120, 40, 0, 1, 1, 0)
            .picker
            .is_some_and(|p| !p.busy && p.total == 2)
    });
    f.app.dispatch(Command::PickerQuery {
        query: "main".into(),
    });
    f.app.dispatch(Command::PickerAccept { index: None });
    assert_eq!(f.cursor(), "def greet\ndef ".len());
    // Workspace symbols query the server as you type.
    f.action("workspace-symbols", "");
    f.app.dispatch(Command::PickerQuery {
        query: "gre".into(),
    });
    f.wait("workspace symbols", |a| {
        a.snapshot(120, 40, 0, 1, 1, 0)
            .picker
            .is_some_and(|p| !p.busy && p.total == 1 && p.items[0].label == "greet")
    });
}

#[test]
fn rename_format_code_actions_and_completion_edit_the_workspace() {
    let mut f = fixture(SOURCE, true);
    f.synced();
    // Rename edits the open document and opens the closed file with its
    // edit applied, unsaved, so the whole rename can be reviewed and undone.
    f.goto(1, 5);
    f.action("rename-symbol", "");
    assert_eq!(f.app.prompt.as_ref().unwrap().input, "greet");
    f.app.dispatch(Command::UpdatePrompt {
        input: "welcome".into(),
        replacement: String::new(),
        case_sensitive: false,
        whole_word: false,
    });
    f.app.dispatch(Command::SubmitPrompt { all: false });
    f.wait("rename", |a| a.status.starts_with("Renamed"));
    assert!(f.text().starts_with("def welcome\n"));
    assert!(f.text().contains("main calls welcome"));
    let other = f.file.with_file_name("other.fk");
    assert_eq!(
        fs::read_to_string(&other).unwrap(),
        "greet from a closed file\n",
        "nothing is written behind the user's back"
    );
    let opened = f
        .app
        .documents
        .values()
        .find(|d| d.path.as_deref() == Some(other.as_path()))
        .expect("the closed file opened");
    assert_eq!(opened.text(), "welcome from a closed file\n");
    assert!(opened.dirty());
    assert!(
        f.app.status.contains("Renamed in 2 files"),
        "{}",
        f.app.status
    );
    // Undo restores every occurrence in one step.
    f.app.dispatch(Command::Undo);
    assert_eq!(f.text(), SOURCE);
    f.app.dispatch(Command::Redo);
    f.synced();

    // Formatting applies the server's edits.
    f.goto(2, 8);
    f.typed("   ");
    f.action("format-document", "");
    f.wait("format", |a| a.status == "Formatted");
    assert!(f.text().contains("def main\n"));
    // Format on save.
    f.app.dispatch(Command::Configure {
        name: "format-on-save".into(),
        value: "true".into(),
    });
    f.goto(2, 8);
    f.typed("  ");
    f.app.dispatch(Command::Save);
    f.wait("formatted save", |a| a.status == "Saved");
    assert!(fs::read_to_string(&f.file).unwrap().contains("def main\n"));

    // A quick fix from a diagnostic, and a command that edits via the server.
    f.synced();
    f.diagnosed();
    f.goto(3, "main calls welcome 👩‍💻 ER".len());
    f.action("code-actions", "");
    f.wait("code actions", |a| {
        a.snapshot(120, 40, 0, 1, 1, 0)
            .picker
            .is_some_and(|p| p.total == 2)
    });
    f.app.dispatch(Command::PickerAccept { index: Some(0) });
    assert!(f.text().contains("👩‍💻 OK"), "{}", f.text());
    f.synced();
    f.action("code-actions", "");
    f.wait("code actions", |a| {
        a.snapshot(120, 40, 0, 1, 1, 0).picker.is_some()
    });
    let picker = f.app.snapshot(120, 40, 0, 1, 1, 0).picker.unwrap();
    let header = picker
        .items
        .iter()
        .position(|i| i.label == "Add a header comment")
        .unwrap();
    f.app.dispatch(Command::PickerAccept {
        index: Some(header),
    });
    f.wait("server edit", |a| {
        a.documents
            .values()
            .any(|d| d.text().starts_with("# header\n"))
    });

    // Completion opens while typing and narrows; Enter accepts.
    f.app.dispatch(Command::Key {
        key: Key {
            key: "End".into(),
            ctrl: true,
            ..Default::default()
        },
    });
    f.typed("wel");
    f.wait("completion", |a| {
        a.snapshot(120, 40, 0, 1, 1, 0)
            .completion
            .is_some_and(|c| c.items.first().is_some_and(|i| i.label == "welcome"))
    });
    f.press("Enter");
    assert!(f.text().ends_with("welcome"), "{:?}", f.text());
    // A snippet completion selects its first field.
    f.press("Enter");
    f.typed("print_l");
    f.wait("snippet completion", |a| {
        a.snapshot(120, 40, 0, 1, 1, 0)
            .completion
            .is_some_and(|c| c.items.first().is_some_and(|i| i.label == "print_line"))
    });
    f.press("Tab");
    assert!(f.text().ends_with("print_line(text)"), "{:?}", f.text());
    let v = &f.app.views[&f.editor()];
    let selected = &f.text()[v.anchor.unwrap()..v.cursor];
    assert_eq!(selected, "text");
    f.typed("\"hi\"");
    f.press("Tab");
    assert!(f.text().ends_with("print_line(\"hi\")"));
    // Escape closes an open list.
    f.typed(" mai");
    f.wait("completion", |a| {
        a.snapshot(120, 40, 0, 1, 1, 0).completion.is_some()
    });
    f.press("Escape");
    assert!(f.app.snapshot(120, 40, 0, 1, 1, 0).completion.is_none());
}

#[test]
fn restricted_workspaces_start_no_language_server() {
    let mut f = fixture(SOURCE, false);
    for _ in 0..30 {
        f.app.process_events();
        thread::sleep(Duration::from_millis(10));
    }
    assert!(f.server_text().is_none());
    assert!(f.app.language_servers().is_empty());
    // Asking for language features asks to trust the project.
    f.action("hover", "");
    assert_eq!(f.app.prompt.as_ref().unwrap().kind, "trust");
    f.app.dispatch(Command::DismissPrompt);
    assert!(f.server_text().is_none());
    // Trusting starts it.
    f.app.set_session_trust(true);
    f.synced();
    assert_eq!(f.app.language_servers(), ["Fake"]);
    f.app.set_session_trust(false);
    f.app.process_events();
    assert!(f.app.language_servers().is_empty());
}

#[test]
fn edits_for_changed_text_or_outside_the_workspace_are_refused() {
    let mut f = fixture(SOURCE, true);
    f.synced();
    f.diagnosed();
    // A code action computed before the user typed is not applied.
    f.goto(3, "main calls greet 👩‍💻 ER".len());
    f.action("code-actions", "");
    f.wait("code actions", |a| {
        a.snapshot(120, 40, 0, 1, 1, 0)
            .picker
            .is_some_and(|p| p.total == 2)
    });
    // The document changes while the list is open.
    let before = f.text();
    let id = f.editor();
    let doc = f.app.views[&id].document;
    f.app
        .documents
        .get_mut(&doc)
        .unwrap()
        .replace(0, 0, "x", 0)
        .unwrap();
    f.app.dispatch(Command::PickerAccept { index: Some(0) });
    assert!(f.app.status.contains("changed since"), "{}", f.app.status);
    assert_eq!(f.text(), format!("x{before}"), "nothing was applied");

    // A rename that would touch a file outside the workspace changes nothing.
    let outside = f.dir.path().join("outside.fk");
    fs::write(&outside, "greet outside\n").unwrap();
    std::env::set_var("FAKE_LSP_EXTRA", &outside);
    f.app.dispatch(Command::Action {
        name: "restart-language-servers".into(),
        argument: String::new(),
    });
    f.synced();
    let before = f.text();
    f.goto(1, 5);
    f.action("rename-symbol", "renamed");
    f.wait("refusal", |a| a.status.contains("outside the workspace"));
    assert_eq!(f.text(), before);
    assert_eq!(fs::read_to_string(&outside).unwrap(), "greet outside\n");
}

#[test]
fn a_format_cut_short_by_a_stopped_server_leaves_the_document_saveable() {
    let mut f = fixture(SOURCE, true);
    f.synced();
    f.app.dispatch(Command::Configure {
        name: "format-on-save".into(),
        value: "true".into(),
    });
    f.goto(2, 8);
    f.typed("   ");
    // Save starts a format; the server goes away before answering.
    f.app.dispatch(Command::Save);
    assert_eq!(f.app.status, "Formatting…");
    f.action("restart-language-servers", "");
    // Format-on-save still saves (unformatted), and saving works again.
    let file = f.file.clone();
    f.wait("save", |a| {
        !a.dirty() && fs::read_to_string(&file).is_ok_and(|text| text.contains("def main   \n"))
    });
    assert!(fs::read_to_string(&f.file)
        .unwrap()
        .contains("def main   \n"));
    f.typed("x");
    f.app.dispatch(Command::Save);
    assert_ne!(f.app.status, "Error: Wait for formatting to finish");
}

#[test]
fn servers_for_projects_outside_trusted_folders_do_not_start() {
    let mut f = fixture(SOURCE, true);
    f.synced();
    // A file from an untrusted folder, opened in this trusted workspace.
    let elsewhere = f.dir.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let file = elsewhere.join("other.fk");
    fs::write(&file, "def x\n").unwrap();
    f.app.dispatch(Command::Open { path: file.clone() });
    f.wait("open", |a| a.status.starts_with("Opened"));
    for _ in 0..30 {
        f.app.process_events();
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !f.dir.path().join("dump/other.fk").exists(),
        "no server saw it"
    );
    f.action("hover", "");
    let prompt = f.app.prompt.clone().expect("asks to trust that folder");
    assert!(prompt.input.contains("elsewhere"), "{}", prompt.input);
}

#[test]
fn initialization_configuration_and_manual_and_triggered_signatures() {
    let mut f = fixture("call\n", true);
    f.synced();
    let dump = f.dir.path().join("dump");
    f.wait("configuration exchange", |_| {
        dump.join("settings.json").exists() && dump.join("configuration.json").exists()
    });
    let read = |name: &str| -> serde_json::Value {
        serde_json::from_slice(&fs::read(dump.join(name)).unwrap()).unwrap()
    };
    let init = read("initialize.json");
    assert_eq!(init["initializationOptions"]["fixture"], true);
    assert_eq!(
        init["capabilities"]["workspace"]["workspaceEdit"]["resourceOperations"],
        serde_json::json!(["create", "rename", "delete"])
    );
    assert_eq!(
        read("settings.json"),
        serde_json::json!({"fake":{"enabled":true}})
    );
    assert_eq!(
        read("configuration.json"),
        serde_json::json!([{"enabled":true}])
    );
    f.action("signature-help", "");
    f.wait("manual signature", |a| {
        a.hover_view().is_some_and(|h| {
            h.text.contains("call(arg: int)") && h.text.contains("Argument documentation")
        })
    });
    f.press("Escape");
    assert!(f.app.hover_view().is_none());
    f.typed("(");
    f.wait("triggered signature", |a| {
        a.hover_view()
            .is_some_and(|h| h.text.contains("call(arg: int)"))
    });
}
