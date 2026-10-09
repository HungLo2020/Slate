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
