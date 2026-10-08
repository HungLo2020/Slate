//! IDE features in the terminal interface, driven through a real
//! pseudo-terminal: the file picker, project search, several carets,
//! folding, and a language server's diagnostics, hover and completion.
mod support;
use std::fs;
use support::{wait_file, Env};

#[test]
fn full_workspace_panes_are_present_in_the_rendered_terminal() {
    let env = Env::new();
    let file = env.path("file.txt");
    fs::write(&file, "hello\n").unwrap();
    let mut tui = env.slate(&["--workspace", file.to_str().unwrap()]);
    // These assertions use the VT parser's reconstructed screen, so cursor
    // movement optimizations cannot make a pane appear to be missing.
    for header in ["files #1", "editor #2", "terminal #3"] {
        tui.wait_for(header);
    }
}

fn project(env: &Env) -> std::path::PathBuf {
    let root = env.path("project");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/util.rs"), "pub fn greet() {}\n").unwrap();
    fs::write(root.join("notes.md"), "# Notes\nsearch me here\n").unwrap();
    root
}

#[test]
fn file_picker_and_project_search() {
    let env = Env::new();
    let root = project(&env);
    let mut tui = env.slate(&[root.join("notes.md").to_str().unwrap()]);
    tui.wait_for("# Notes");
    // Ctrl+P opens Go to file.
    tui.send(b"\x10");
    tui.wait_for("Go to file");
    tui.send("utl");
    tui.wait_for("src/util.rs");
    tui.send(b"\r");
    tui.wait_gone("Go to file");
    tui.wait_for("pub fn greet");
    // Search in files from the palette.
    tui.command("search-in-files");
    tui.wait_for("Search in files");
    tui.send("search me");
    tui.wait_for("1 result in 1 file");
    tui.wait_for("notes.md:2");
    tui.send(b"\r");
    tui.wait_gone("Search in files");
    tui.wait_for("search me here");
}

#[test]
fn carets_and_folds_in_the_terminal() {
    let env = Env::new();
    let path = env.path("code.rs");
    fs::write(&path, "fn a() {\n    one;\n    two;\n}\nlet x = x + x;\n").unwrap();
    let mut tui = env.slate(&[path.to_str().unwrap()]);
    tui.wait_for("fn a()");
    // Fold the function from the palette; its body disappears.
    tui.command("fold");
    tui.wait_gone("one;");
    tui.wait_for("⋯");
    tui.command("unfold");
    tui.wait_for("one;");
    // Ctrl+D twice selects two occurrences of `x`, then typing replaces both.
    // Ctrl+End, up a line, Home, then right onto the first `x`.
    tui.send(b"\x1b[1;5F");
    tui.send(b"\x1b[A");
    tui.send(b"\x1b[H");
    for _ in 0..4 {
        tui.send(b"\x1b[C");
    }
    tui.send(b"\x04");
    tui.send(b"\x04");
    tui.send("y");
    tui.send(b"\x13");
    wait_file(&path, b"fn a() {\n    one;\n    two;\n}\nlet y = y + x;\n");
}

#[test]
fn language_server_diagnostics_hover_and_completion() {
    let env = Env::new();
    let root = env.path("lsp");
    fs::create_dir_all(&root).unwrap();
    let server = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("crates/slate-core/tests/fixtures/fake_lsp.py");
    fs::create_dir_all(env.path("config/slate")).unwrap();
    fs::write(
        env.path("config/slate/languages.toml"),
        format!(
            "[[language]]\nname = \"Fake\"\nextensions = [\"fk\"]\nserver = [\"python3\", \"{}\"]\n",
            server.display()
        ),
    )
    .unwrap();
    // Trust the folder so its language server may start.
    fs::write(
        env.path("config/slate/trusted-folders"),
        format!("{}\n", root.canonicalize().unwrap().display()),
    )
    .unwrap();
    let file = root.join("main.fk");
    fs::write(&file, "def greet\ngreet ERROR\n").unwrap();
    let mut tui = env.slate(&[file.to_str().unwrap()]);
    tui.wait_for("def greet");
    tui.wait_for("✖ 1 ⚠ 0");
    // The problem under the caret is named in the status line.
    tui.send(b"\x1b[B");
    tui.send(b"\x1b[F");
    tui.wait_for("error: an error here");
    // Alt+H shows hover text.
    tui.send(b"\x1b[H");
    tui.send(b"\x1bh");
    tui.wait_for("hover for greet");
    tui.send(b"\x1b");
    tui.wait_gone("hover for greet");
    // Typing opens completions; Tab accepts.
    tui.send(b"\x1b[F");
    tui.send(" gr");
    tui.wait_for("Tab/Enter accepts");
    tui.send(b"\t");
    tui.wait_gone("Tab/Enter accepts");
    tui.send(b"\x13");
    wait_file(&file, b"def greet\ngreet ERROR greet\n");
}
