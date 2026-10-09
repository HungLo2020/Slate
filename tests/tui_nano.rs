//! End-to-end checks of the terminal interface as a nano replacement: the
//! real `slate` binary runs in a pseudo-terminal and is driven by keystrokes.
mod support;
use std::fs;
use support::{wait_file, Env, Tui};

#[test]
fn missing_files_are_created_on_first_save_and_clean_quit_needs_no_prompt() {
    let env = Env::new();
    let path = env.path("brand-new.txt");
    let mut tui = env.slate(&[path.to_str().unwrap()]);
    tui.wait_for("brand-new.txt");
    assert!(!path.exists(), "Opening must not create the file");
    tui.send(b"\x11");
    assert_eq!(tui.exit_code(), 0);
    assert!(!path.exists());

    let mut tui = env.slate(&[path.to_str().unwrap()]);
    tui.send("hello");
    tui.send(b"\x13");
    wait_file(&path, b"hello");
    tui.send(b"\x11");
    assert_eq!(tui.exit_code(), 0);
}

#[test]
fn plus_line_column_positions_the_cursor() {
    let env = Env::new();
    let path = env.path("lines.txt");
    fs::write(&path, "one\ntwo\nthree\n").unwrap();
    let mut tui = env.slate(&["+3,2", path.to_str().unwrap()]);
    tui.wait_for("Ln 3, Col 2");
    tui.send("X");
    tui.send(b"\x13");
    wait_file(&path, b"one\ntwo\ntXhree\n");
}

#[test]
fn standard_input_becomes_an_untitled_buffer_saved_through_a_prompt() {
    let env = Env::new();
    let target = env.path("from-stdin.txt");
    let mut command = env.command("/bin/sh");
    command.args([
        "-c",
        &format!(
            "printf 'piped\\r\\nline\\r\\n' | '{}' -",
            env!("CARGO_BIN_EXE_slate")
        ),
    ]);
    let mut tui = Tui::spawn(command, 100, 30);
    tui.wait_for("piped");
    tui.wait_for("CRLF");
    tui.send(b"\x13");
    tui.wait_for("Save to a new file path");
    tui.send(target.to_str().unwrap());
    tui.send(b"\r");
    // Line endings round-trip: the input used CRLF.
    wait_file(&target, b"piped\r\nline\r\n");
}

#[test]
fn view_mode_refuses_edits() {
    let env = Env::new();
    let path = env.path("ro.txt");
    fs::write(&path, "keep\n").unwrap();
    let mut tui = env.slate(&["-v", path.to_str().unwrap()]);
    tui.wait_for("Read-only");
    tui.send("typed");
    tui.wait_for("open read-only");
    tui.send(b"\x11");
    assert_eq!(
        tui.exit_code(),
        0,
        "Read-only view must quit without a prompt"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "keep\n");
}

#[test]
fn pasted_carriage_returns_become_lines() {
    let env = Env::new();
    let path = env.path("paste.txt");
    fs::write(&path, "base\n").unwrap();
    let mut tui = env.slate(&[path.to_str().unwrap()]);
    tui.paste("a1\rb2\r");
    tui.send(b"\x13");
    wait_file(&path, b"a1\nb2\nbase\n");
}

#[test]
fn crlf_latin1_and_bom_files_keep_their_format() {
    let env = Env::new();
    let crlf = env.path("dos.txt");
    fs::write(&crlf, b"x\r\ny\r\n").unwrap();
    let mut tui = env.slate(&[crlf.to_str().unwrap()]);
    tui.wait_for("CRLF");
    // End, then type: text lands before the line break, never inside it.
    tui.send(b"\x1b[F");
    tui.send("Z");
    tui.send(b"\r");
    tui.send("new");
    tui.send(b"\x13");
    wait_file(&crlf, b"xZ\r\nnew\r\ny\r\n");
    drop(tui);

    let latin = env.path("latin.txt");
    fs::write(&latin, b"caf\xe9\n").unwrap();
    let mut tui = env.slate(&[latin.to_str().unwrap()]);
    tui.wait_for("windows-1252");
    tui.wait_for("café");
    tui.send(b"\x1b[F");
    tui.send(" ok");
    tui.send(b"\x13");
    wait_file(&latin, b"caf\xe9 ok\n");
    drop(tui);

    let bom = env.path("bom.txt");
    fs::write(&bom, b"\xEF\xBB\xBFhi\n").unwrap();
    let mut tui = env.slate(&[bom.to_str().unwrap()]);
    tui.wait_for("UTF-8 BOM");
    tui.send("!");
    tui.send(b"\x13");
    wait_file(&bom, b"\xEF\xBB\xBF!hi\n");
}

#[test]
fn binary_files_are_refused() {
    let env = Env::new();
    let path = env.path("bin.dat");
    fs::write(&path, b"\x7fELF\0\0\x01").unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_slate"))
        .arg(&path)
        .env("XDG_CONFIG_HOME", env.path("config"))
        .env("XDG_STATE_HOME", env.path("state"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("binary"));
}

#[cfg(unix)]
#[test]
fn read_only_files_ask_before_saving_and_keep_links_and_mode() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let env = Env::new();
    let path = env.path("locked.txt");
    let link = env.path("link.txt");
    fs::write(&path, "data\n").unwrap();
    fs::hard_link(&path, &link).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
    let inode = fs::metadata(&path).unwrap().ino();
    let mut tui = env.slate(&[path.to_str().unwrap()]);
    tui.wait_for("[RO]");
    tui.send("new ");
    tui.send(b"\x13");
    tui.wait_for("is read-only");
    tui.send("n");
    tui.wait_gone("overwrite it anyway");
    assert_eq!(fs::read_to_string(&path).unwrap(), "data\n");
    tui.send(b"\x13");
    tui.wait_for("overwrite it anyway");
    tui.send("y");
    wait_file(&path, b"new data\n");
    let meta = fs::metadata(&path).unwrap();
    assert_eq!(meta.mode() & 0o777, 0o444);
    assert_eq!(meta.ino(), inode, "The hard link was broken");
    assert_eq!(fs::read_to_string(&link).unwrap(), "new data\n");
}

#[test]
fn quit_with_unsaved_changes_offers_save_discard_and_cancel() {
    let env = Env::new();
    let path = env.path("q.txt");
    fs::write(&path, "v1\n").unwrap();
    let mut tui = env.slate(&[path.to_str().unwrap()]);
    tui.send("A");
    tui.send(b"\x11");
    tui.wait_for("Save changes before quitting?");
    tui.send(b"\x1b");
    tui.wait_gone("Save changes before quitting?");
    tui.send(b"\x11");
    tui.wait_for("Save changes before quitting?");
    tui.send("y");
    assert_eq!(tui.exit_code(), 0);
    assert_eq!(fs::read_to_string(&path).unwrap(), "Av1\n");

    let mut tui = env.slate(&[path.to_str().unwrap()]);
    tui.send("B");
    tui.send(b"\x11");
    tui.wait_for("Save changes before quitting?");
    tui.send("n");
    assert_eq!(tui.exit_code(), 0);
    assert_eq!(fs::read_to_string(&path).unwrap(), "Av1\n");
}

#[test]
fn nano_keymap_cut_paste_justify_and_exit() {
    let env = Env::new();
    env.config("keymap = \"nano\"\nwrap_column = 20\n");
    let path = env.path("nano.txt");
    fs::write(&path, "first\nsecond\nthird\n").unwrap();
    let mut tui = env.slate(&[path.to_str().unwrap()]);
    tui.wait_for("^K Cut");
    // ^K twice collects both lines; ^U pastes them back after moving down.
    tui.send(b"\x0b");
    tui.send(b"\x0b");
    tui.send(b"\x1b[B");
    tui.send(b"\x15");
    tui.send(b"\x0f"); // ^O writes out
    wait_file(&path, b"third\nfirst\nsecond\n");
    // ^J justifies the paragraph at the wrap column.
    tui.send(b"\x1b[H");
    tui.send(b"\x1b[1;5H"); // Ctrl+Home
    tui.send("alpha beta gamma delta epsilon ");
    tui.send(b"\x0a");
    tui.send(b"\x0f");
    wait_file(
        &path,
        b"alpha beta gamma\ndelta epsilon third\nfirst second\n",
    );
    // ^W searches, ^C reports the position, M-D counts words.
    tui.send(b"\x17");
    tui.wait_for("Find");
    tui.send("second\r");
    tui.send(b"\x1b"); // closes the prompt
    tui.send(b"\x1b"); // clears the found selection
    tui.send(b"\x03");
    tui.wait_for("line 3/4");
    tui.send(b"\x1bd");
    tui.wait_for("Document: 3 lines, 8 words");
    tui.send(b"\x18"); // ^X exits
    assert_eq!(tui.exit_code(), 0);
}

#[test]
fn soft_wrap_shows_and_navigates_long_lines() {
    let env = Env::new();
    env.config("soft_wrap = true\nline_numbers = false\n");
    let path = env.path("wrap.txt");
    let long = format!("{} END", "word ".repeat(40));
    fs::write(&path, format!("{long}\nnext\n")).unwrap();
    let mut tui = env.slate(&[path.to_str().unwrap()]);
    tui.wait_for("Wrap");
    let screen = tui.screen();
    assert!(
        screen.contains(" END"),
        "The wrapped tail is visible:\n{screen}"
    );
    assert!(!screen.contains("next") || screen.find("END") < screen.find("next"));
    // Down moves within the wrapped line, not to the next logical line.
    tui.send(b"\x1b[B");
    tui.send("|");
    tui.send(b"\x13");
    tui.wait_for("Saved"); // Saving is asynchronous; wait for completion before inspecting disk.
    let saved = fs::read_to_string(&path).unwrap();
    let first_line = saved.lines().next().unwrap();
    assert!(
        first_line.contains('|') && saved.contains("\nnext\n"),
        "{saved}"
    );
}

#[test]
fn spell_check_and_backups() {
    let env = Env::new();
    env.config("backup = true\n");
    let path = env.path("spell.txt");
    fs::write(&path, "This is teh text\n").unwrap();
    let mut tui = env.slate(&[path.to_str().unwrap()]);
    if slate_core::fsio::which("aspell").is_some()
        || slate_core::fsio::which("hunspell").is_some()
        || slate_core::fsio::which("enchant-2").is_some()
    {
        tui.command("spell-check");
        tui.wait_for("misspelled: teh");
    }
    if slate_core::fsio::which("aspell").is_some()
        || slate_core::fsio::which("hunspell").is_some()
        || slate_core::fsio::which("enchant-2").is_some()
    {
        // The misspelling is selected; typing replaces it.
        tui.send("the");
    } else {
        tui.command("find teh");
        tui.send("the");
    }
    tui.send(b"\x13");
    wait_file(&path, b"This is the text\n");
    assert_eq!(
        fs::read(env.path("spell.txt~")).unwrap(),
        b"This is teh text\n"
    );
}

#[test]
fn colour_depth_follows_the_terminal() {
    let env = Env::new();
    let path = env.path("c.rs");
    fs::write(&path, "fn main() {}\n").unwrap();
    // Indexed colours used by each mode: (sequence prefix, largest index).
    let indexed = |raw: &str| -> Vec<u32> {
        raw.split("8;5;")
            .skip(1)
            .filter_map(|tail| {
                tail.split(|c: char| !c.is_ascii_digit())
                    .next()?
                    .parse()
                    .ok()
            })
            .collect()
    };
    for (vars, forbidden, required, max_index) in [
        (
            vec![("NO_COLOR", "1")],
            vec!["38;2;", "38;5;", "48;5;"],
            vec![],
            0,
        ),
        (
            vec![("COLORTERM", ""), ("TERM", "linux")],
            vec!["38;2;"],
            vec![],
            15,
        ),
        (vec![("COLORTERM", "")], vec!["38;2;"], vec!["38;5;"], 255),
        (vec![], vec![], vec!["38;2;"], 255),
    ] {
        let mut command = env.command(env!("CARGO_BIN_EXE_slate"));
        command.arg(&path);
        for (name, value) in &vars {
            command.env(name, value);
        }
        let mut tui = Tui::spawn(command, 80, 20);
        tui.wait_for("fn main");
        for r in &required {
            tui.wait_raw(0, r);
        }
        let raw = String::from_utf8_lossy(&tui.raw()).into_owned();
        for f in forbidden {
            assert!(!raw.contains(f), "{vars:?} produced {f}");
        }
        for r in required {
            assert!(raw.contains(r), "{vars:?} lacks {r}");
        }
        assert!(
            indexed(&raw).iter().all(|i| *i <= max_index),
            "{vars:?} used colours beyond {max_index}"
        );
    }
}

#[test]
fn mouse_capture_toggles() {
    let env = Env::new();
    env.config("keymap = \"nano\"\n");
    let path = env.path("m.txt");
    fs::write(&path, "x\n").unwrap();
    let mut tui = env.slate(&[path.to_str().unwrap()]);
    assert!(String::from_utf8_lossy(&tui.raw()).contains("\x1b[?1000h"));
    tui.send(b"\x1bm"); // M-M
    tui.wait_raw(0, "\x1b[?1000l");
    assert!(fs::read_to_string(env.path("config/slate/settings.toml"))
        .unwrap()
        .contains("tui_mouse = false"));
}

#[cfg(target_os = "linux")]
#[test]
fn suspend_stops_the_process_and_resume_redraws() {
    let env = Env::new();
    env.config("keymap = \"nano\"\n");
    let path = env.path("s.txt");
    fs::write(&path, "x\n").unwrap();
    let mut tui = env.slate(&[path.to_str().unwrap()]);
    let pid = tui.pid();
    tui.send(b"\x1a"); // ^Z
    let state = || {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        stat.rsplit(')')
            .next()
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .to_string()
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while state() != "T" {
        assert!(
            std::time::Instant::now() < deadline,
            "not stopped: {}",
            state()
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let before = tui.raw().len();
    unsafe { libc::kill(pid as i32, libc::SIGCONT) };
    // Keys typed before Slate retakes the terminal go through cooked mode,
    // so wait for it to return to its screen.
    tui.wait_raw(before, "\x1b[?1049h");
    tui.send("y");
    tui.send(b"\x0f");
    wait_file(&path, b"yx\n");
    tui.wait_for("Write Out");
}

#[cfg(unix)]
#[test]
fn permission_denied_offers_a_sudo_save() {
    // A stand-in sudo records what it was asked to write; the real file
    // (owned by root) is never touched.
    use std::os::unix::fs::PermissionsExt;
    let target = std::path::Path::new("/etc/hostname");
    if unsafe { libc::geteuid() } == 0 || !target.exists() || slate_core::fsio::writable(target) {
        return;
    }
    let env = Env::new();
    let bin = env.path("bin");
    fs::create_dir(&bin).unwrap();
    let record = env.path("sudo-record");
    let sudo = bin.join("sudo");
    fs::write(
        &sudo,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{0}.args'\ncat > '{0}.data'\n",
            record.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&sudo, fs::Permissions::from_mode(0o755)).unwrap();
    let mut command = env.command(env!("CARGO_BIN_EXE_slate"));
    command.arg(target);
    command.env(
        "PATH",
        format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
    );
    let mut tui = Tui::spawn(command, 100, 30);
    tui.send("edited-");
    tui.send(b"\x13");
    tui.wait_for("save with sudo");
    tui.send("y");
    tui.wait_for("with sudo");
    let data = fs::read_to_string(env.path("sudo-record.data")).unwrap();
    let (header, data) = data.split_once('\n').unwrap();
    let baseline: slate_core::fsio::Baseline = serde_json::from_str(header).unwrap();
    assert_eq!(
        baseline,
        slate_core::fsio::Baseline::of(&fs::read(target).unwrap())
    );
    assert!(data.starts_with("edited-"), "{data}");
    let args = fs::read_to_string(env.path("sudo-record.args")).unwrap();
    assert!(
        args.contains("--internal-elevated-save") && args.contains("/etc/hostname"),
        "{args}"
    );
    tui.wait_gone(" *");
    assert!(!fs::read_to_string(target).unwrap().starts_with("edited-"));
}

#[cfg(unix)]
#[test]
fn desktop_clipboard_tools_are_used_when_a_display_exists() {
    use std::os::unix::fs::PermissionsExt;
    let env = Env::new();
    let bin = env.path("bin");
    fs::create_dir(&bin).unwrap();
    let copied = env.path("copied");
    for (name, body) in [
        (
            "wl-copy",
            format!("#!/bin/sh\ncat > '{}'\n", copied.display()),
        ),
        ("wl-paste", "#!/bin/sh\nprintf 'FROM_DESKTOP'\n".to_string()),
    ] {
        let tool = bin.join(name);
        fs::write(&tool, body).unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = env.path("clip.txt");
    fs::write(&path, "abc").unwrap();
    let mut command = env.command(env!("CARGO_BIN_EXE_slate"));
    command.arg(&path);
    command.env("WAYLAND_DISPLAY", "wayland-test");
    command.env(
        "PATH",
        format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
    );
    let mut tui = Tui::spawn(command, 100, 30);
    tui.send(b"\x01"); // select all
    tui.send(b"\x03"); // copy
    wait_file(&copied, b"abc");
    tui.send(b"\x16"); // paste reads the desktop clipboard
    tui.send(b"\x13");
    wait_file(&path, b"FROM_DESKTOP");
}

#[test]
fn several_files_open_as_tabs() {
    let env = Env::new();
    let a = env.path("first.txt");
    let b = env.path("second.txt");
    fs::write(&a, "A").unwrap();
    fs::write(&b, "B").unwrap();
    let mut tui = env.slate(&[a.to_str().unwrap(), b.to_str().unwrap()]);
    tui.wait_for("first.txt");
    tui.wait_for("second.txt");
    tui.send(b"\x1b[18~"); // F7: next tab
    tui.send("2");
    tui.send(b"\x13");
    wait_file(&b, b"2B");
}

#[test]
fn external_changes_reload_clean_files_and_ask_about_modified_ones() {
    let env = Env::new();
    let path = env.path("watched.txt");
    fs::write(&path, "first\n").unwrap();
    let mut tui = env.slate(&[path.to_str().unwrap()]);
    fs::write(&path, "second\n").unwrap();
    tui.wait_for("reloaded");
    tui.wait_for("second");
    tui.send("mine ");
    fs::write(&path, "third version\n").unwrap();
    tui.wait_for("changed on disk while you have unsaved changes");
    tui.send("r");
    tui.wait_gone("changed on disk while");
    tui.wait_for("third version");
    // Reloading keeps the cursor where it was; type at the start instead.
    tui.send(b"\x1b[1;5H");
    tui.send("again ");
    fs::write(&path, "fourth\n").unwrap();
    tui.wait_for("changed on disk while you have unsaved changes");
    tui.send("k");
    tui.wait_for("Kept your version");
    tui.send(b"\x13");
    wait_file(&path, b"again third version\n");
}
