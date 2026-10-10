//! End-to-end core boundaries shared by the native GUI and terminal dispatchers.
use crate::{cli::Launch, document::Document, preferences::StartupMode, App, Command, EditorView};
use serde_json::json;
use std::{
    path::Path,
    time::{Duration, Instant},
};
fn app(root: &Path) -> App {
    App::launch(
        &Launch {
            directory: Some(root.into()),
            startup: Some(StartupMode::EditorOnly),
            recover: false,
            ..Default::default()
        },
        None,
    )
    .unwrap()
}
fn setup() -> (tempfile::TempDir, std::sync::MutexGuard<'static, ()>) {
    let guard = crate::paths::TEST_ENV
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
    std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
    std::fs::create_dir(dir.path().join("workspace")).unwrap();
    (dir, guard)
}
fn content(a: &mut App, text: &str) -> (u64, u64) {
    let view = a.active_editor().unwrap();
    let doc = a.views[&view].document;
    let mut d = Document::scratch();
    d.replace(0, 0, text, 0).unwrap();
    a.documents.insert(doc, d);
    a.views.insert(
        view,
        EditorView {
            document: doc,
            ..Default::default()
        },
    );
    (doc, view)
}
fn drain(a: &mut App, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done(a) && Instant::now() < deadline {
        a.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(done(a), "Timed out: {}", a.status);
}
#[test]
fn regex_capture_replacement_is_scoped_and_undoable() {
    let (dir, _guard) = setup();
    let mut a = app(&dir.path().join("workspace"));
    let (doc, view) = content(&mut a, "x=12\ny=34\nx=56\n");
    a.views.get_mut(&view).unwrap().anchor = Some(0);
    a.views.get_mut(&view).unwrap().cursor = 10;
    a.execute(Command::ConfigureSearch {
        regex: true,
        selection_only: true,
    })
    .unwrap();
    a.execute(Command::Search {
        query: r"(?P<name>[xy])=(\d+)".into(),
        case_sensitive: true,
        whole_word: false,
        backward: false,
    })
    .unwrap();
    // Changing regex controls must retain the original region, not the found match.
    let scope = a.search.scope;
    a.execute(Command::ConfigureSearch {
        regex: true,
        selection_only: true,
    })
    .unwrap();
    assert_eq!(a.search.scope, scope);
    a.replace_matches("${name}:$2 $$", true).unwrap();
    assert_eq!(a.documents[&doc].text(), "x:12 $\ny:34 $\nx=56\n");
    a.documents.get_mut(&doc).unwrap().undo(0).unwrap();
    assert_eq!(a.documents[&doc].text(), "x=12\ny=34\nx=56\n");
}
#[test]
fn replacement_budget_refuses_partial_changes_and_large_results_are_stale_safe() {
    let (dir, _guard) = setup();
    let mut a = app(&dir.path().join("workspace"));
    let (doc, _) = content(&mut a, &"x ".repeat(10_001));
    a.set_search("x".into(), true, false);
    assert!(a.replace_matches("y", true).is_err());
    assert_eq!(a.documents[&doc].text(), "x ".repeat(10_001));
    content(&mut a, &("prefix ".repeat(50_000) + "TARGET"));
    a.set_search("TARGET".into(), true, false);
    let started = Instant::now();
    a.find(false).unwrap();
    assert!(started.elapsed() < Duration::from_millis(100));
    assert!(a.document_search_state.busy);
    a.documents
        .get_mut(&doc)
        .unwrap()
        .replace(0, 0, "new", 0)
        .unwrap();
    drain(&mut a, |a| !a.document_search_state.busy);
    assert!(a.status.contains("changed"));
    a.find(false).unwrap();
    a.document_search_state.cancel();
    a.views.get_mut(&a.active_editor().unwrap()).unwrap().cursor = 0;
    std::thread::sleep(Duration::from_millis(100));
    a.poll();
    assert_eq!(a.views[&a.active_editor().unwrap()].cursor, 0);
}
#[test]
fn rectangular_selection_respects_tabs_unicode_and_short_lines() {
    let (dir, _guard) = setup();
    let mut a = app(&dir.path().join("workspace"));
    let (doc, view) = content(&mut a, "a界bc\n\txyz\na\n123456\n");
    let to = a.documents[&doc].line_offset(3) + 4;
    a.select_rectangle(view, 1, to).unwrap();
    assert_eq!(a.selections_text(view), "界b\n\t\n\n234"); // short line clamps at its end
    a.multi_paste(view, "Z").unwrap();
    assert_eq!(a.documents[&doc].text(), "aZc\nZxyz\naZ\n1Z56\n");
    a.documents.get_mut(&doc).unwrap().undo(0).unwrap();
    assert_eq!(a.documents[&doc].text(), "a界bc\n\txyz\na\n123456\n");
}
#[test]
fn rectangular_selection_handles_lines_ending_in_multibyte_characters() {
    let (dir, _guard) = setup();
    let mut a = app(&dir.path().join("workspace"));
    let (doc, view) = content(&mut a, "abé\nab界\nab😀\nabcd");
    let to = a.documents[&doc].line_offset(3) + 4;
    a.select_rectangle(view, 2, to).unwrap();
    assert_eq!(a.selections_text(view), "é\n界\n😀\ncd");
}
#[test]
fn split_views_of_one_document_are_both_highlighted() {
    let (dir, _guard) = setup();
    let mut a = app(&dir.path().join("workspace"));
    let (doc, view) = content(&mut a, &"fn f() {}\n".repeat(3000));
    a.command_line("split-right");
    let other = a.active_editor().unwrap();
    assert_ne!(other, view);
    assert_eq!(a.views[&other].document, doc);
    a.views.get_mut(&other).unwrap().top = 2500;
    let current = |a: &App, line| a.line_spans(doc, line).is_some_and(|(_, c)| c);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !(current(&a, 0) && current(&a, 2500)) {
        assert!(Instant::now() < deadline, "both views highlighted");
        a.process_events();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        !current(&a, 1500),
        "lines between distant views are not requested"
    );
}
#[test]
fn many_carets_move_other_views_folds_and_history_once() {
    let (dir, _guard) = setup();
    let mut a = app(&dir.path().join("workspace"));
    let (doc, view) = content(&mut a, &"x\n".repeat(10_000));
    a.command_line("split-right");
    let other = a.active_editor().unwrap();
    {
        let o = a.views.get_mut(&other).unwrap();
        o.cursor = 2 * 7_000 + 1;
        o.anchor = Some(2 * 7_000);
        o.folds = vec![2 * 5_000];
    }
    a.back.push((doc, 2 * 9_000));
    let carets = (0..10_000)
        .map(|line| crate::cursors::Selection::caret(2 * line))
        .collect();
    a.set_selections(view, carets);
    let key = crate::Key {
        key: "a".into(),
        text: "ab".into(),
        ..Default::default()
    };
    assert!(a.multi_key(view, &key).unwrap());
    assert_eq!(a.documents[&doc].text(), "abx\n".repeat(10_000));
    let o = &a.views[&other];
    // Each line gained two bytes before these positions.
    assert_eq!((o.cursor, o.anchor), (4 * 7_000 + 3, Some(4 * 7_000)));
    assert_eq!(o.folds, [4 * 5_000]);
    assert_eq!(a.back.last(), Some(&(doc, 4 * 9_000)));
    assert_eq!(a.selections(view).len(), 10_000);
    assert!(a.selections(view).iter().all(|s| s.cursor % 4 == 2));
}
#[test]
fn editorconfig_save_is_async_preserves_format_and_shutdown_waits_for_write() {
    let (dir, _guard) = setup();
    let root = dir.path().join("workspace");
    std::fs::write(root.join(".editorconfig"),"root=true\n[*]\nindent_style=space\nindent_size=2\ntab_width=8\nend_of_line=crlf\ncharset=latin1\ntrim_trailing_whitespace=true\ninsert_final_newline=true\n").unwrap();
    let path = root.join("a.txt");
    std::fs::write(&path, "café  ").unwrap();
    let mut a = app(&root);
    let doc = *a.documents.keys().next().unwrap();
    a.documents.insert(doc, Document::open(&path).unwrap());
    let options = a.preferences_for_document(doc);
    assert_eq!(options.indent_width, 2);
    assert_eq!(options.tab_width, 8);
    a.start_save(doc, None, false, false).unwrap();
    assert!(a.saves.pending_save.contains(&doc));
    a.flush_workspace().unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"caf\xe9\r\n");
    assert_eq!(a.documents[&doc].text(), "café\n");
    assert!(!a.documents[&doc].dirty());
    a.documents.get_mut(&doc).unwrap().undo(0).unwrap();
    assert_eq!(a.documents[&doc].text(), "café  ");
}
#[test]
fn editorconfig_encoding_failure_changes_neither_buffer_nor_disk() {
    let (dir, _guard) = setup();
    let root = dir.path().join("workspace");
    std::fs::write(
        root.join(".editorconfig"),
        "[*]\ncharset=latin1\ntrim_trailing_whitespace=true\n",
    )
    .unwrap();
    let path = root.join("a.txt");
    std::fs::write(&path, "€  ").unwrap();
    let mut a = app(&root);
    let doc = *a.documents.keys().next().unwrap();
    a.documents.insert(doc, Document::open(&path).unwrap());
    a.start_save(doc, None, false, false).unwrap();
    a.flush_workspace().unwrap();
    assert!(a.status.contains("cannot store"));
    assert_eq!(a.documents[&doc].text(), "€  ");
    assert_eq!(std::fs::read_to_string(path).unwrap(), "€  ");
}
#[test]
fn workspace_switch_cancel_and_gui_request_preserve_unsaved_documents() {
    let (dir, _guard) = setup();
    let root = dir.path().join("workspace");
    let dest = dir.path().join("other");
    std::fs::create_dir(&dest).unwrap();
    let mut a = app(&root);
    let (doc, _) = content(&mut a, "keep me");
    a.open_workspace(dest.clone()).unwrap();
    assert_eq!(a.prompt.as_ref().unwrap().kind, "switch-workspace");
    a.execute(Command::DismissPrompt).unwrap();
    assert!(a.pending_workspace.is_none());
    assert_eq!(a.root, root);
    assert_eq!(a.documents[&doc].text(), "keep me");
    a.terminal_frontend = false;
    a.open_workspace(dest.clone()).unwrap();
    assert_eq!(a.root, root);
    assert_eq!(
        a.frontend_requests.last().unwrap(),
        &format!("open-workspace:{}", dest.display())
    );
    a.terminal_frontend = true;
    a.open_workspace(dest.clone()).unwrap();
    a.submit_prompt(true).unwrap();
    assert_eq!(a.root, dest);
    assert!(a.documents.values().all(|d| d.text() != "keep me"));
}
#[test]
fn ordered_resource_edits_create_rename_and_edit_without_saving_text() {
    let (dir, _guard) = setup();
    let root = dir.path().join("workspace");
    let mut a = app(&root);
    let created = root.join("created.txt");
    let renamed = root.join("renamed.txt");
    let edits = json!({"documentChanges":[{"kind":"create","uri":crate::lsp::uri(&created)},{"kind":"rename","oldUri":crate::lsp::uri(&created),"newUri":crate::lsp::uri(&renamed)},{"textDocument":{"uri":crate::lsp::uri(&renamed),"version":null},"edits":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"newText":"unsaved"}]}]});
    a.apply_workspace_edit(&edits, None).unwrap();
    assert!(!created.exists());
    assert_eq!(std::fs::read_to_string(&renamed).unwrap(), "");
    let id = a.document_for(&renamed).unwrap();
    assert_eq!(a.documents[&id].text(), "unsaved");
    assert!(a.documents[&id].dirty());
    let outside = dir.path().join("outside.txt");
    let invalid = json!({"documentChanges":[{"kind":"delete","uri":crate::lsp::uri(&renamed)},{"kind":"create","uri":crate::lsp::uri(&outside)}]});
    assert!(a.apply_workspace_edit(&invalid, None).is_err());
    assert!(renamed.exists());
    assert!(!outside.exists());
}
#[test]
fn resource_failure_rolls_back_prior_disk_operations() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let (dir, _guard) = setup();
        let root = dir.path().join("workspace");
        let blocked = root.join("blocked");
        std::fs::create_dir(&blocked).unwrap();
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o555)).unwrap();
        let first = root.join("first.txt");
        let mut a = app(&root);
        let edit = json!({"documentChanges":[{"kind":"create","uri":crate::lsp::uri(&first)},{"kind":"create","uri":crate::lsp::uri(&blocked.join("no.txt"))}]});
        assert!(a.apply_workspace_edit(&edit, None).is_err());
        assert!(!first.exists());
        std::fs::set_permissions(blocked, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}
#[test]
fn conflict_choices_are_undoable_and_malformed_markers_are_refused() {
    let (dir, _guard) = setup();
    let mut a = app(&dir.path().join("workspace"));
    let original =
        "before\n<<<<<<< ours\nlocal\n||||||| base\nold\n=======\nremote\n>>>>>>> theirs\nafter\n";
    let (doc, view) = content(&mut a, original);
    a.views.get_mut(&view).unwrap().cursor = 10;
    a.resolve_conflict("both").unwrap();
    assert_eq!(a.documents[&doc].text(), "before\nlocal\nremote\nafter\n");
    a.documents.get_mut(&doc).unwrap().undo(0).unwrap();
    assert_eq!(a.documents[&doc].text(), original);
    content(&mut a, "<<<<<<< ours\nlocal\n>>>>>>> theirs\n");
    assert!(a.resolve_conflict("ours").is_err());
}
#[test]
fn histories_shortcuts_recent_projects_and_locks_remain_private_and_bounded() {
    let (dir, _guard) = setup();
    let root = dir.path().join("workspace");
    let mut a = app(&root);
    a.configure("history-log", "true").unwrap();
    a.prompt = Some(crate::search::Prompt {
        kind: "find".into(),
        input: "needle".into(),
        ..Default::default()
    });
    let p = a.prompt.clone().unwrap();
    a.remember_prompt_history(&p);
    a.prompt.as_mut().unwrap().input = "draft".into();
    a.search_history(true).unwrap();
    assert_eq!(a.prompt.as_ref().unwrap().input, "needle");
    a.search_history(false).unwrap();
    assert_eq!(a.prompt.as_ref().unwrap().input, "draft");
    a.set_shortcut("editor", "Ctrl+Shift+s", "save-all")
        .unwrap();
    assert_eq!(a.preferences.editor_keys["Ctrl+Shift+s"], "save-all");
    assert!(a
        .set_shortcut("editor", "Ctrl+Shift+s", "nonexistent")
        .is_err());
    a.remember_project();
    assert_eq!(
        crate::desktop::load_projects().first(),
        Some(&root.to_string_lossy().into_owned())
    );
    let path = root.join("file.txt");
    std::fs::write(&path, "text").unwrap();
    let doc = *a.documents.keys().next().unwrap();
    a.documents.insert(doc, Document::open(&path).unwrap());
    a.configure("locking", "true").unwrap();
    a.update_file_locks();
    let mut b = app(&root);
    let docb = *b.documents.keys().next().unwrap();
    b.documents.insert(docb, Document::open(&path).unwrap());
    b.update_file_locks();
    assert!(b.status.contains("another Slate"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(crate::paths::state_dir().join("slate/history.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
#[test]
fn project_search_scope_reports_skipped_files_and_explicit_inclusions() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".ignore"), "ignored.txt\n").unwrap();
    for name in ["shown.rs", "shown.txt", ".hidden.txt", "ignored.txt"] {
        std::fs::write(dir.path().join(name), "needle\n").unwrap();
    }
    std::fs::write(dir.path().join("binary.txt"), b"needle\0").unwrap();
    std::fs::write(dir.path().join("large.txt"), "needle long long long").unwrap();
    let options = crate::project::SearchOptions {
        query: "needle".into(),
        ..Default::default()
    };
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let mut hits = Vec::new();
    let mut policy = crate::project::SearchPolicy {
        max_bytes: 16,
        ..Default::default()
    };
    let report = crate::project::search_with_policy(
        dir.path(),
        &options,
        &policy,
        &std::collections::HashMap::new(),
        &cancel,
        0,
        |batch| {
            hits.extend(batch);
            true
        },
    )
    .unwrap();
    assert_eq!(report.matches, 2);
    assert_eq!(report.large, 1);
    assert_eq!(report.binary, 1);
    assert!(hits
        .iter()
        .all(|h| !h.path.ends_with("ignored.txt") && !h.path.ends_with(".hidden.txt")));
    policy.hidden = true;
    policy.ignored = true;
    policy.include = "*.txt".into();
    policy.exclude = "shown.txt;large.txt;binary.txt".into();
    let report = crate::project::search_with_policy(
        dir.path(),
        &options,
        &policy,
        &std::collections::HashMap::new(),
        &cancel,
        0,
        |_| true,
    )
    .unwrap();
    assert_eq!(report.matches, 2);
    policy.include = "[invalid".into();
    assert!(policy.validate().is_err());
}
#[test]
fn git_hunk_staging_keeps_other_hunks_and_rejects_changed_previews() {
    use std::process::Command as Process;
    let dir = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        let output = Process::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    run(&["init", "-q"]);
    run(&["config", "user.email", "test@example.invalid"]);
    run(&["config", "user.name", "Test"]);
    let original = (0..30).map(|n| format!("line {n}\n")).collect::<String>();
    let path = dir.path().join("file.txt");
    std::fs::write(&path, &original).unwrap();
    run(&["add", "file.txt"]);
    run(&["commit", "-qm", "fixture"]);
    let changed = original
        .replace("line 1\n", "first\n")
        .replace("line 25\n", "second\n");
    std::fs::write(&path, &changed).unwrap();
    let context = crate::git::Context {
        root: dir.path().into(),
        scope: None,
    };
    let patch =
        crate::git::load_patch(&context, std::ffi::OsStr::new("file.txt"), false, false).unwrap();
    assert_eq!(patch.lines().filter(|l| l.starts_with("@@ ")).count(), 2);
    let preview = crate::comparison::Preview {
        context: context.clone(),
        path: "file.txt".into(),
        display: "file.txt".into(),
        staged: false,
        untracked: false,
        patch,
        side_by_side: false,
    };
    crate::comparison::apply_hunk(&preview, 0).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), changed);
    let staged =
        crate::git::load_patch(&context, std::ffi::OsStr::new("file.txt"), true, false).unwrap();
    assert!(staged.contains("+first"));
    assert!(!staged.contains("+second"));
    assert!(crate::comparison::apply_hunk(&preview, 1)
        .unwrap_err()
        .contains("changed"));
    let unstage = crate::comparison::Preview {
        patch: staged,
        staged: true,
        ..preview
    };
    crate::comparison::apply_hunk(&unstage, 0).unwrap();
    assert!(
        crate::git::load_patch(&context, std::ffi::OsStr::new("file.txt"), true, false)
            .unwrap()
            .is_empty()
    );
}
#[test]
fn launch_compatibility_flags_are_validated_and_do_not_persist() {
    let (dir, _guard) = setup();
    let path = dir.path().join("workspace/a.txt");
    std::fs::write(&path, "text").unwrap();
    let args = [
        "-I",
        "-B",
        "-S",
        "-E",
        "-H",
        "-G",
        "--tabsize=8",
        path.to_str().unwrap(),
    ];
    let launch = crate::cli::parse(args.iter().map(std::ffi::OsString::from)).unwrap();
    let a = App::launch(&launch, None).unwrap();
    assert!(
        a.preferences.backup
            && a.preferences.soft_wrap
            && a.preferences.history_log
            && a.preferences.locking
    );
    assert_eq!(a.preferences.tab_width, 8);
    assert!(!crate::preferences::Preferences::path().exists());
    for args in [["-T", "0"], ["--tabsize=99", "a.txt"], ["-T", "oops"]] {
        assert!(crate::cli::parse(args.map(std::ffi::OsString::from)).is_err());
    }
}
#[test]
fn streaming_encoders_round_trip_chunk_boundaries_and_reject_unmappable_text() {
    for encoding in [
        "UTF-8",
        "UTF-16LE",
        "UTF-16BE",
        "ISO-8859-1",
        "windows-1252",
        "ISO-2022-JP",
    ] {
        let text = if encoding == "ISO-2022-JP" {
            "日本語\n".repeat(4000)
        } else {
            "café\n".repeat(4000)
        };
        let format = crate::text_format::TextFormat {
            encoding: encoding.into(),
            line_ending: crate::text_format::LineEnding::Crlf,
            ..Default::default()
        };
        let bytes =
            crate::text_format::encode_rope(&ropey::Rope::from_str(&text), &format).unwrap();
        let decoded = crate::text_format::decode(&bytes, Some(encoding)).unwrap();
        assert_eq!(decoded.text, text);
        crate::text_format::validate_rope(&ropey::Rope::from_str(&text), &format).unwrap();
    }
    let latin = crate::text_format::TextFormat {
        encoding: "ISO-8859-1".into(),
        ..Default::default()
    };
    assert!(crate::text_format::validate_rope(&ropey::Rope::from_str("€"), &latin).is_err());
}
#[test]
fn resource_overwrite_of_open_clean_file_preserves_buffer_and_disk_contract() {
    let (dir, _guard) = setup();
    let root = dir.path().join("workspace");
    let path = root.join("existing.txt");
    std::fs::write(&path, "original").unwrap();
    let mut a = app(&root);
    let doc = *a.documents.keys().next().unwrap();
    a.documents.insert(doc, Document::open(&path).unwrap());
    let edit = json!({"documentChanges":[{"kind":"create","uri":crate::lsp::uri(&path),"options":{"overwrite":true}},{"textDocument":{"uri":crate::lsp::uri(&path)},"edits":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"newText":"new"}]}]});
    a.apply_workspace_edit(&edit, None).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
    assert_eq!(a.documents[&doc].text(), "new");
    assert!(a.documents[&doc].dirty());
    a.start_save(doc, None, false, false).unwrap();
    a.flush_workspace().unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
    assert!(!a.documents[&doc].dirty());
}
#[test]
fn long_project_matches_have_bounded_previews_even_for_whitespace() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("long.txt"), " ".repeat(100_000)).unwrap();
    let options = crate::project::SearchOptions {
        query: r" +".into(),
        regex: true,
        ..Default::default()
    };
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let mut hits = Vec::new();
    crate::project::search(
        dir.path(),
        &options,
        &std::collections::HashMap::new(),
        &cancel,
        0,
        |batch| {
            hits.extend(batch);
            true
        },
    )
    .unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].preview.len() < 512);
    assert_eq!(hits[0].length, 100_000);
}

#[test]
fn diagnostics_drain_latest_error_on_immediate_shutdown() {
    let (dir, _guard) = setup();
    let mut a = app(&dir.path().join("workspace"));
    for n in 0..150 {
        a.status = format!("Error: fixture {n}");
        a.record_diagnostic_status();
    }
    a.flush_workspace().unwrap();
    let entries: Vec<String> = crate::user_state::read("diagnostics.json");
    assert_eq!(entries.len(), 100);
    assert!(entries.last().unwrap().ends_with("fixture 149"));
    a.support_report();
    let report = &a.documents[&a.views[&a.active_editor().unwrap()].document];
    assert!(report.read_only);
    assert!(report.text().contains("fixture 149"));
}

/// A layout with no room anywhere: every pane at the depth limit holds the
/// most tabs a pane may have.
fn saturated(a: &mut App, depth: usize, tab: &crate::layout::View) -> crate::layout::Node {
    use crate::layout::{Axis, Node, MAX_DEPTH, MAX_TABS};
    if depth == MAX_DEPTH {
        return Node::Pane {
            id: a.id(),
            tabs: vec![tab.clone(); MAX_TABS],
            active: 0,
        };
    }
    Node::Split {
        id: a.id(),
        axis: Axis::Vertical,
        ratio: 0.5,
        first: Box::new(saturated(a, depth + 1, tab)),
        second: Box::new(saturated(a, depth + 1, tab)),
    }
}
#[test]
fn opening_into_a_full_layout_leaves_other_tabs_alone() {
    use crate::layout::View;
    let (dir, _guard) = setup();
    let root = dir.path().join("workspace");
    let mut a = app(&root);
    let (_, view) = content(&mut a, "one\ntwo\n");
    a.views.get_mut(&view).unwrap().cursor = 2;
    a.layout = saturated(&mut a, 0, &View::Files);
    let pane = a.layout.panes()[0];
    a.layout.pane_mut(pane).unwrap().0[0] = View::Editor(view);
    a.focus = pane;
    let path = root.join("more.txt");
    std::fs::write(&path, "more\n").unwrap();
    let path = path.canonicalize().unwrap();
    a.pending_positions
        .insert(path.clone(), (1, crate::navigation::Column::Byte(0)));
    let documents = a.documents.len();
    assert!(a.finish_open(pane, Document::open(&path).unwrap()).is_err());
    assert!(a.status.starts_with("Too many tabs"), "{}", a.status);
    assert_eq!(a.documents.len(), documents);
    assert!(a.document_for(&path).is_none());
    assert!(a.pending_positions.is_empty());
    assert_eq!(a.views[&view].cursor, 2);
}
#[test]
fn layout_changes_keep_a_tab_for_unsaved_documents() {
    use crate::layout::View;
    let (dir, _guard) = setup();
    let mut a = app(&dir.path().join("workspace"));
    let (doc, view) = content(&mut a, "unsaved");
    assert!(a.documents[&doc].dirty());
    let (clean, clean_view) = (a.id(), a.id());
    a.documents.insert(clean, Document::scratch());
    a.views.insert(
        clean_view,
        EditorView {
            document: clean,
            ..Default::default()
        },
    );
    // Only shells and one clean document, everywhere, at every limit.
    a.layout = saturated(&mut a, 0, &View::Terminal(999));
    let pane = a.layout.panes()[7];
    a.layout.pane_mut(pane).unwrap().0[3] = View::Editor(clean_view);
    a.adopt_orphans();
    assert_eq!(a.layout.tabs(pane).unwrap()[3], View::Editor(view));
    assert!(a.documents[&doc].dirty());
    assert!(!a.documents.contains_key(&clean));
    assert!(!a.views.contains_key(&clean_view));
    assert!(a.layout.validate());
}
#[test]
fn layout_changes_never_close_buffers_that_would_be_lost() {
    use crate::layout::View;
    let (dir, _guard) = setup();
    let root = dir.path().join("workspace");
    let path = root.join("waited.txt");
    std::fs::write(&path, "waited\n").unwrap();
    let mut a = launch_files(&root, 0);
    a.execute(Command::Open { path: path.clone() }).unwrap();
    drain(&mut a, |a| a.document_for(&path).is_some());
    let waited = a.document_for(&path).unwrap();
    let (_client, server) = std::os::unix::net::UnixStream::pair().unwrap();
    a.attach_editor_wait(
        crate::instance::WaitTicket::new(server).unwrap(),
        &[crate::cli::LaunchFile {
            path: path.clone(),
            line: None,
            column: None,
        }],
    )
    .unwrap();
    // Read-only standard input never counts as dirty, but has no file.
    let (stdin, stdin_view) = (a.id(), a.id());
    let mut input = Document::from_bytes(b"piped log", "standard input").unwrap();
    input.read_only = true;
    a.documents.insert(stdin, input);
    a.views.insert(
        stdin_view,
        EditorView {
            document: stdin,
            ..Default::default()
        },
    );
    let waited_view = *a
        .views
        .iter()
        .find(|(_, v)| v.document == waited)
        .unwrap()
        .0;
    let doc = a.id();
    let mut unsaved = Document::scratch();
    unsaved.replace(0, 0, "unsaved", 0).unwrap();
    a.documents.insert(doc, unsaved);
    a.layout = saturated(&mut a, 0, &View::Terminal(999));
    let pane = a.layout.panes()[3];
    a.layout.pane_mut(pane).unwrap().0[0] = View::Editor(stdin_view);
    a.layout.pane_mut(pane).unwrap().0[1] = View::Editor(waited_view);
    a.adopt_orphans();
    assert_eq!(a.layout.tabs(pane).unwrap()[0], View::Editor(stdin_view));
    assert_eq!(a.layout.tabs(pane).unwrap()[1], View::Editor(waited_view));
    assert_eq!(a.documents[&stdin].text(), "piped log");
    assert!(a.documents.contains_key(&waited));
    // The unsaved buffer could not be shown, but stays open.
    assert!(a.documents[&doc].dirty());
    assert!(a.status.starts_with("Too many tabs"), "{}", a.status);
}
fn launch_files(root: &Path, count: usize) -> App {
    let files = (0..count)
        .map(|n| {
            let path = root.join(format!("file{n}.txt"));
            std::fs::write(&path, format!("{n}\n")).unwrap();
            crate::cli::LaunchFile {
                path,
                line: None,
                column: None,
            }
        })
        .collect();
    App::launch(
        &Launch {
            files,
            recover: false,
            ..Default::default()
        },
        None,
    )
    .unwrap()
}
#[test]
fn launching_many_files_keeps_the_session_recoverable() {
    let (dir, _guard) = setup();
    let root = dir.path().join("workspace");
    let a = launch_files(&root, 40);
    assert_eq!(a.documents.len(), 40);
    assert!(a.layout.validate());
    let shown = a.layout.views();
    assert!(a
        .views
        .keys()
        .all(|id| shown.contains(&crate::layout::View::Editor(*id))));
    crate::recovery_io::write(&dir.path().join("forty.json"), &a.workspace()).unwrap();
    drop(a);
    let a = launch_files(&root, 130);
    assert_eq!(a.documents.len(), crate::workspace::MAX_DOCUMENTS);
    assert!(a.status.contains("Opened 128 of 130 files"), "{}", a.status);
    assert!(a.layout.validate());
    crate::recovery_io::write(&dir.path().join("many.json"), &a.workspace()).unwrap();
}
#[test]
fn documents_are_found_by_any_path_to_their_file() {
    let (dir, _guard) = setup();
    let root = dir.path().join("workspace");
    let mut a = app(&root);
    let (doc, _) = content(&mut a, "text");
    let real = root.join("real.txt");
    a.execute(Command::SaveAs {
        path: real.clone(),
        overwrite: false,
    })
    .unwrap();
    drain(&mut a, |a| !a.documents[&doc].dirty());
    let link = root.join("link.txt");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(a.document_for(&link), Some(doc));
    assert_eq!(a.document_for(&root.join("./real.txt")), Some(doc));
    // Save As moves the lookup to the new file.
    let moved = root.join("moved.txt");
    a.execute(Command::SaveAs {
        path: moved.clone(),
        overwrite: false,
    })
    .unwrap();
    drain(&mut a, |a| {
        a.documents[&doc].path.as_deref() == Some(&*moved.canonicalize().unwrap_or_default())
    });
    assert_eq!(a.document_for(&moved), Some(doc));
    assert_eq!(a.document_for(&link), None);
    let view = a.active_editor().unwrap();
    a.close_tab(a.focus, view, true).unwrap();
    assert_eq!(a.document_for(&moved), None);
}
#[test]
fn closing_a_document_forgets_what_was_cached_for_it() {
    let (dir, _guard) = setup();
    let mut a = app(&dir.path().join("workspace"));
    let text = "fn a() {\n    one;\n}\n".repeat(2000);
    let (doc, view) = content(&mut a, &text);
    a.views.get_mut(&view).unwrap().folds = vec![0];
    assert!(!a.hidden_lines(view).is_empty());
    a.overview_for(doc);
    assert!(a.overview_pending.contains_key(&doc) && a.overview_workers.contains_key(&doc));
    a.snapshot(100, 30, 1, 1, 1, 3);
    assert!(a.render_cache.contains_key(&view));
    a.close_tab(a.focus, view, true).unwrap();
    assert!(!a.documents.contains_key(&doc) && !a.views.contains_key(&view));
    assert!(!a.fold_cache.lock().unwrap().contains_key(&view));
    assert!(!a.render_cache.contains_key(&view));
    assert!(!a.overview_cache.contains_key(&doc));
    assert!(!a.overview_pending.contains_key(&doc));
    assert!(!a.overview_workers.contains_key(&doc));
    assert!(!a.highlights.contains_key(&doc) && !a.highlight_pending.contains_key(&doc));
}
#[test]
fn editor_signatures_follow_carets_and_nothing_else() {
    let (dir, _guard) = setup();
    let mut a = app(&dir.path().join("workspace"));
    let (_, view) = content(&mut a, "one two three\n");
    let before = a.editor_signature(view);
    assert_eq!(a.editor_signature(view), before);
    a.views
        .get_mut(&view)
        .unwrap()
        .extra
        .push(crate::cursors::Selection::caret(4));
    let one = a.editor_signature(view);
    assert_ne!(one, before);
    a.views.get_mut(&view).unwrap().extra[0].anchor = Some(7);
    assert_ne!(a.editor_signature(view), one);
    a.views.get_mut(&view).unwrap().extra.clear();
    assert_eq!(a.editor_signature(view), before);
    a.views.get_mut(&view).unwrap().cursor = 3;
    assert_ne!(a.editor_signature(view), before);
    // Fields drawn from neither the view nor the screen leave it alone.
    a.views.get_mut(&view).unwrap().cursor = 0;
    a.views.get_mut(&view).unwrap().goal = Some(9);
    assert_eq!(a.editor_signature(view), before);
}
#[test]
fn editor_signatures_follow_the_path_and_diff_markers() {
    let (dir, _guard) = setup();
    let mut a = app(&dir.path().join("workspace"));
    let (doc, view) = content(&mut a, "one\n");
    let before = a.editor_signature(view);
    // Diagnostics are looked up by path: Save As must redraw.
    a.documents.get_mut(&doc).unwrap().path = Some(dir.path().join("workspace/saved.rs"));
    let saved = a.editor_signature(view);
    assert_ne!(saved, before);
    // Diff hunk markers show once a document becomes an inspection.
    a.show_comparison(crate::comparison::Preview {
        context: crate::git::Context {
            root: dir.path().into(),
            scope: None,
        },
        path: "file.txt".into(),
        display: "file.txt".into(),
        staged: false,
        untracked: false,
        patch: "@@ -1 +1 @@\n-a\n+b\n".into(),
        side_by_side: false,
    });
    let inspection = a.diff_inspections.values().next().unwrap().clone();
    a.diff_inspections.insert(doc, inspection);
    assert_ne!(a.editor_signature(view), saved);
}
#[test]
fn open_documents_stay_within_what_recovery_holds() {
    let (dir, _guard) = setup();
    let root = dir.path().join("workspace");
    let mut a = launch_files(&root, crate::workspace::MAX_DOCUMENTS);
    a.execute(Command::New).unwrap();
    assert!(
        a.status.starts_with("Too many open documents"),
        "{}",
        a.status
    );
    let path = root.join("one-more.txt");
    std::fs::write(&path, "more\n").unwrap();
    assert!(a
        .finish_open(a.focus, Document::open(&path).unwrap())
        .is_err());
    assert_eq!(a.documents.len(), crate::workspace::MAX_DOCUMENTS);
    a.workspace().validate(&a.root).unwrap();
}
#[test]
fn salvage_keeps_unsaved_buffers_before_clean_references() {
    let (dir, _guard) = setup();
    let root = dir.path().join("workspace");
    let mut a = launch_files(&root, 2);
    let mut w = a.workspace();
    let template = w.documents.values().next().unwrap().recovery_copy();
    assert!(template.reference);
    // More clean references than a checkpoint holds, with the one unsaved
    // buffer last in id order.
    for id in 1000..1000 + crate::workspace::MAX_DOCUMENTS as u64 {
        w.documents.insert(id, template.recovery_copy());
    }
    let mut unsaved = Document::scratch();
    unsaved.replace(0, 0, "unsaved work", 0).unwrap();
    let last = a.id() + 10_000;
    w.documents.insert(last, unsaved.recovery_copy());
    let bytes = serde_json::to_vec(&w).unwrap();
    let salvaged = crate::workspace::Workspace::salvage(&bytes, &root).unwrap();
    assert_eq!(salvaged.documents.len(), crate::workspace::MAX_DOCUMENTS);
    assert_eq!(salvaged.documents[&last].text(), "unsaved work");
}
