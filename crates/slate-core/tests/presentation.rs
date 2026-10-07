use slate_core::{
    document::{self, Document},
    layout::Axis,
    App, Command, Key,
};
use std::{
    fs, process,
    sync::Arc,
    time::{Duration, Instant},
};
fn snapshot(app: &mut App) -> slate_core::Snapshot {
    app.snapshot(160, 45, 1, 1, 1, 3)
}
fn git(root: &std::path::Path, args: &[&str]) {
    let output = process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn git_ready(app: &mut App) -> slate_core::Snapshot {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let observed = app.events().generation();
        app.process_events();
        let view = snapshot(app);
        if !view.git_busy {
            return view;
        }
        assert!(
            Instant::now() < deadline,
            "Git did not complete: {}",
            view.status
        );
        app.events().wait(observed, Duration::from_millis(100));
    }
}
#[test]
fn indexed_positions_follow_edits_and_history() {
    let mut doc = Document::from_text("a\n猫\n\t👩‍💻\r\n".into()).unwrap();
    let mut seed = 7u64;
    for _ in 0..250 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let boundaries: Vec<_> = doc
            .text()
            .char_indices()
            .map(|(i, _)| i)
            .chain([doc.text().len()])
            .collect();
        let start = boundaries[seed as usize % boundaries.len()];
        let end = boundaries[(seed >> 32) as usize % boundaries.len()];
        let (start, end) = (start.min(end), start.max(end));
        let value = ["\n猫\n", "x", "", "\t界", "👩‍💻\r\n"][seed as usize % 5];
        doc.replace(start, end, value, start).unwrap();
        for history in 0..3 {
            if history == 1 {
                doc.undo(0);
            } else if history == 2 {
                doc.redo(0);
            }
            assert_eq!(
                doc.line_count(),
                doc.text().bytes().filter(|b| *b == b'\n').count() + 1
            );
            for row in 0..doc.line_count() + 1 {
                assert_eq!(
                    doc.line_offset(row),
                    if row == 0 {
                        0
                    } else {
                        doc.text()
                            .match_indices('\n')
                            .nth(row - 1)
                            .map(|(i, _)| i + 1)
                            .unwrap_or(doc.text().len())
                    }
                );
                for col in 0..12 {
                    assert_eq!(
                        doc.at_line_col(row, col, 4),
                        document::at_line_col_with_tabs(doc.text(), row, col, 4)
                    );
                }
            }
            for (byte, _) in doc.text().char_indices() {
                assert_eq!(
                    doc.line_col(byte, 4),
                    document::line_col_with_tabs(doc.text(), byte, 4)
                );
            }
        }
    }
}
#[test]
fn typing_groups_break_at_navigation_and_save_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("edit.txt");
    fs::write(&path, "").unwrap();
    let mut app = App::new(&path).unwrap();
    for c in ["a", "猫", "b"] {
        app.dispatch(Command::Key {
            key: Key {
                key: c.into(),
                text: c.into(),
                ..Default::default()
            },
        });
    }
    app.dispatch(Command::Undo);
    assert_eq!(app.documents[&10].text(), "");
    assert!(!app.dirty());
    app.dispatch(Command::Redo);
    assert_eq!(app.documents[&10].text(), "a猫b");
    app.dispatch(Command::Key {
        key: Key {
            key: "Left".into(),
            ..Default::default()
        },
    });
    app.dispatch(Command::Key {
        key: Key {
            key: "x".into(),
            text: "x".into(),
            ..Default::default()
        },
    });
    app.dispatch(Command::Undo);
    assert_eq!(app.documents[&10].text(), "a猫b");
    app.dispatch(Command::Save);
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.dirty() {
        app.process_events();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    app.dispatch(Command::Undo);
    assert!(app.dirty());
    app.dispatch(Command::Redo);
    assert!(!app.dirty());
}
#[test]
fn unchanged_and_unrelated_views_reuse_their_screens() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("edit.txt");
    fs::write(&path, "hello\n").unwrap();
    let mut app = App::new(&path).unwrap();
    app.dispatch(Command::Split {
        axis: Axis::Vertical,
        kind: None,
    });
    // Await syntax completion before checking that presentation work remains stable.
    let deadline = Instant::now() + Duration::from_millis(350);
    while Instant::now() < deadline {
        app.process_events();
        std::thread::sleep(Duration::from_millis(5));
    }
    let a = snapshot(&mut app);
    let b = snapshot(&mut app);
    let revision = app.revision() - app.events().generation();
    for _ in 0..20 {
        app.dispatch(Command::Pointer {
            pane: 3,
            row: 5,
            col: 5,
            kind: "move".into(),
            button: 3,
            shift: false,
            ctrl: false,
            alt: false,
        });
    }
    assert_eq!(
        app.revision() - app.events().generation(),
        revision,
        "Passive terminal hover must not invalidate presentation or typing groups"
    );
    for pane in a.panes.iter().filter(|p| p.kind == "editor") {
        assert!(Arc::ptr_eq(
            pane.screen.as_ref().unwrap(),
            b.panes
                .iter()
                .find(|p| p.id == pane.id)
                .unwrap()
                .screen
                .as_ref()
                .unwrap()
        ));
    }
    app.dispatch(Command::Key {
        key: Key {
            key: "Right".into(),
            ..Default::default()
        },
    });
    let c = snapshot(&mut app);
    for pane in b.panes.iter().filter(|p| p.kind == "editor") {
        let same = Arc::ptr_eq(
            pane.screen.as_ref().unwrap(),
            c.panes
                .iter()
                .find(|p| p.id == pane.id)
                .unwrap()
                .screen
                .as_ref()
                .unwrap(),
        );
        assert_eq!(
            same,
            pane.id != app.focus,
            "only the moved cursor should invalidate its view"
        );
    }
}
#[test]
fn background_results_and_terminal_output_wake_without_rendering() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opened.txt");
    fs::write(&path, "ASYNC_OPEN").unwrap();
    let mut app = App::new(dir.path()).unwrap();
    let events = app.events();
    app.dispatch(Command::Open { path });
    let deadline = Instant::now() + Duration::from_secs(5);
    while !app.documents.values().any(|d| d.text() == "ASYNC_OPEN") {
        let observed = events.generation();
        app.process_events();
        assert!(Instant::now() < deadline);
        events.wait(observed, Duration::from_millis(100));
    }
    let terminal = app.terminals.get_mut(&12).unwrap();
    let before = terminal.revision();
    terminal.write(b"printf 'WAKE_FROM_PTY\n'\r").unwrap();
    while app.terminals[&12].revision() <= before + 1 {
        let observed = events.generation();
        events.wait(observed, Duration::from_millis(100));
        assert!(Instant::now() < deadline);
    }
    assert!(events.generation() > 0);
}
#[test]
fn shared_actions_offer_context_and_argument_forms() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(dir.path()).unwrap();
    let actions = app.command_catalog("open file");
    assert!(actions
        .iter()
        .any(|c| c.id == "open" && c.enabled && c.argument == "Path"));
    app.dispatch(Command::InvokeAction {
        id: "open".into(),
        argument: String::new(),
    });
    assert_eq!(app.prompt.as_ref().unwrap().kind, "open");
    app.dispatch(Command::DismissPrompt);
    app.dispatch(Command::Focus { pane: 1 });
    let undo = app
        .command_catalog("undo")
        .into_iter()
        .find(|c| c.id == "undo")
        .unwrap();
    assert!(!undo.enabled);
    assert!(!undo.reason.is_empty());
    app.command_line("unknown-command");
    assert!(app.status.contains("F1"));
    assert!(app.status.len() < 120);
}
#[test]
fn git_separates_index_worktree_and_untracked_previews() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    git(
        dir.path(),
        &["config", "user.email", "test@example.invalid"],
    );
    git(dir.path(), &["config", "user.name", "Test"]);
    fs::write(dir.path().join("tracked.txt"), "base\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-qm", "base"]);
    fs::write(dir.path().join("tracked.txt"), "staged\n").unwrap();
    git(dir.path(), &["add", "tracked.txt"]);
    fs::write(dir.path().join("tracked.txt"), "working\n").unwrap();
    fs::write(dir.path().join("new file.txt"), "untracked content\n").unwrap();
    let mut app = App::new(dir.path()).unwrap();
    let view = git_ready(&mut app);
    assert!(view.git_repository);
    assert!(!view.git_branch.is_empty());
    assert_eq!(
        view.git.iter().filter(|e| e.path == "tracked.txt").count(),
        2
    );
    assert!(view.git.iter().any(|e| e.group == "Staged" && e.staged));
    assert!(view.git.iter().any(|e| e.group == "Unstaged" && !e.staged));
    assert!(view.git.iter().any(|e| e.untracked));
    for (path, staged, expected, absent) in [
        ("tracked.txt", true, "+staged", "+working"),
        ("tracked.txt", false, "+working", "-base"),
        ("new file.txt", false, "+untracked content", "never-in-diff"),
    ] {
        app.dispatch(Command::GitDiff {
            path: path.into(),
            staged,
        });
        git_ready(&mut app);
        let id = app
            .views
            .values()
            .map(|v| v.document)
            .filter(|id| app.documents[id].read_only)
            .max()
            .unwrap();
        let doc = &app.documents[&id];
        assert!(doc.text().contains(expected), "{}", doc.text());
        assert!(!doc.text().contains(absent));
        assert!(!doc.dirty());
        app.dispatch(Command::Paste {
            text: "MUTATION".into(),
        });
        assert!(app.status.contains("read-only"));
        app.dispatch(Command::CloseDocument { force: false });
        assert!(!app.dirty());
    }
}
#[test]
fn input_method_replacements_use_utf16_and_inspections_reject_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("unicode.txt");
    fs::write(&path, "a😀b").unwrap();
    let mut app = App::new(&path).unwrap();
    app.views.get_mut(&11).unwrap().cursor = "a😀".len();
    app.dispatch(Command::InputMethod {
        text: "猫".into(),
        replace_start: -2,
        replace_length: 2,
    });
    assert_eq!(app.documents[&10].text(), "a猫b");
    app.dispatch(Command::Undo);
    assert_eq!(app.documents[&10].text(), "a😀b");
    let mut doc = Document::inspection("Preview".into(), "read only".into());
    assert!(doc.replace(0, 0, "bad", 0).is_err());
    assert!(doc.save(Some(&dir.path().join("bad.txt"))).is_err());
    assert!(!doc.dirty());
    assert!(doc.undo(0).is_none());
}

#[test]
fn git_handles_initial_commits_renames_and_failed_operations() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    git(
        dir.path(),
        &["config", "user.email", "test@example.invalid"],
    );
    git(dir.path(), &["config", "user.name", "Test"]);
    fs::write(dir.path().join("original name.txt"), "initial content\n").unwrap();
    git(dir.path(), &["add", "."]);
    let mut app = App::new(dir.path()).unwrap();
    git_ready(&mut app);
    app.dispatch(Command::GitDiff {
        path: "original name.txt".into(),
        staged: true,
    });
    let view = git_ready(&mut app);
    assert!(view.git_error.is_empty(), "{}", view.git_error);
    assert!(app
        .documents
        .values()
        .any(|d| d.read_only && d.text().contains("+initial content")));
    app.dispatch(Command::GitUnstage {
        path: "original name.txt".into(),
    });
    let view = git_ready(&mut app);
    assert!(view.git_error.is_empty(), "{}", view.git_error);
    assert!(view.git.iter().any(|e| e.untracked));
    assert!(dir.path().join("original name.txt").exists());
    app.dispatch(Command::GitStage {
        path: "original name.txt".into(),
    });
    git_ready(&mut app);
    app.dispatch(Command::GitCommit {
        message: "Initial commit".into(),
    });
    assert!(git_ready(&mut app).git_error.is_empty());
    git(dir.path(), &["mv", "original name.txt", "renamed file.txt"]);
    app.dispatch(Command::Refresh);
    let view = git_ready(&mut app);
    assert!(view.git.iter().any(|e| e.path == "renamed file.txt"
        && e.original_path.as_deref() == Some("original name.txt")
        && e.staged));
    app.dispatch(Command::GitUnstage {
        path: "renamed file.txt".into(),
    });
    assert!(git_ready(&mut app).git_error.is_empty());
    assert!(process::Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["diff", "--cached", "--quiet"])
        .status()
        .unwrap()
        .success());
    app.dispatch(Command::GitStage {
        path: "missing file.txt".into(),
    });
    let view = git_ready(&mut app);
    assert!(!view.git_busy);
    assert!(!view.git_error.is_empty());
    assert!(dir.path().join("renamed file.txt").exists());
}

#[cfg(unix)]
#[test]
fn blocked_git_hook_does_not_block_file_browsing_or_editing() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    git(
        dir.path(),
        &["config", "user.email", "test@example.invalid"],
    );
    git(dir.path(), &["config", "user.name", "Test"]);
    fs::write(dir.path().join("edit.txt"), "editor stays responsive\n").unwrap();
    git(dir.path(), &["add", "."]);
    let hook = dir.path().join(".git/hooks/pre-commit");
    fs::write(&hook, "#!/bin/sh\ntouch hook-entered\nn=0\nwhile [ ! -f hook-release ] && [ $n -lt 200 ]; do sleep 0.02; n=$((n+1)); done\n").unwrap();
    fs::set_permissions(hook, fs::Permissions::from_mode(0o700)).unwrap();
    let mut app = App::new(dir.path()).unwrap();
    git_ready(&mut app);
    app.dispatch(Command::GitCommit {
        message: "Slow hook".into(),
    });
    let deadline = Instant::now() + Duration::from_secs(3);
    while !dir.path().join("hook-entered").exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    fs::write(dir.path().join("newly-listed.txt"), "browse independently").unwrap();
    app.dispatch(Command::Refresh);
    app.dispatch(Command::Open {
        path: dir.path().join("edit.txt"),
    });
    loop {
        app.process_events();
        let view = snapshot(&mut app);
        if view.files.iter().any(|e| e.name == "newly-listed.txt")
            && app
                .documents
                .values()
                .any(|d| d.text() == "editor stays responsive\n")
        {
            assert!(
                view.git_busy,
                "The blocked Git hook finished before the file services responded"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "File services blocked behind Git"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    app.dispatch(Command::Paste {
        text: "edited while Git is busy".into(),
    });
    assert!(app
        .documents
        .values()
        .any(|d| d.text().contains("edited while Git is busy")));
    fs::write(dir.path().join("hook-release"), "").unwrap();
    assert!(git_ready(&mut app).git_error.is_empty());
}

#[test]
fn bulk_git_actions_cover_the_repository_and_preserve_working_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "test@example.invalid"]);
    git(root, &["config", "user.name", "Test"]);
    fs::create_dir(root.join("nested")).unwrap();
    for path in [
        "outside.txt",
        "deleted.txt",
        "old name.txt",
        "nested/edit.txt",
    ] {
        fs::write(root.join(path), "base\n").unwrap();
    }
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "base"]);
    git(root, &["mv", "old name.txt", "new name.txt"]);
    fs::remove_file(root.join("deleted.txt")).unwrap();
    fs::write(root.join("outside.txt"), "outside change\n").unwrap();
    fs::write(root.join("nested/edit.txt"), "staged\n").unwrap();
    git(root, &["add", "nested/edit.txt"]);
    fs::write(root.join("nested/edit.txt"), "working\n").unwrap();
    fs::write(root.join("new file.txt"), "untracked\n").unwrap();
    // The application can be opened below the repository root. Bulk actions
    // still include outside.txt, the rename, the deletion and untracked files.
    let mut app = App::new(&root.join("nested")).unwrap();
    git_ready(&mut app);
    app.command_line("stage-all");
    let view = git_ready(&mut app);
    assert!(view.git_error.is_empty(), "{}", view.git_error);
    assert_eq!(view.git.len(), 5);
    assert!(view.git.iter().all(|e| e.staged));
    let staged = process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["show", ":nested/edit.txt"])
        .output()
        .unwrap();
    assert_eq!(staged.stdout, b"working\n");
    app.command_line("unstage-all");
    let view = git_ready(&mut app);
    assert!(view.git_error.is_empty(), "{}", view.git_error);
    assert!(view.git.iter().all(|e| !e.staged));
    assert!(process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["diff", "--cached", "--quiet"])
        .status()
        .unwrap()
        .success());
    assert_eq!(
        fs::read_to_string(root.join("nested/edit.txt")).unwrap(),
        "working\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("outside.txt")).unwrap(),
        "outside change\n"
    );
    assert!(root.join("new name.txt").exists());
    assert!(!root.join("old name.txt").exists());
    assert!(!root.join("deleted.txt").exists());
}

#[test]
fn staging_a_section_does_not_stage_other_sections() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "test@example.invalid"]);
    git(root, &["config", "user.name", "Test"]);
    fs::write(root.join("tracked.txt"), "base\n").unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "base"]);
    fs::write(root.join("tracked.txt"), "working\n").unwrap();
    fs::write(root.join("new file.txt"), "new\n").unwrap();
    let mut app = App::new(root).unwrap();
    git_ready(&mut app);
    app.dispatch(Command::GitStageGroup {
        group: "Untracked".into(),
    });
    let view = git_ready(&mut app);
    assert!(view.git_error.is_empty(), "{}", view.git_error);
    assert!(view
        .git
        .iter()
        .any(|e| e.path == "new file.txt" && e.staged));
    assert!(view
        .git
        .iter()
        .any(|e| e.path == "tracked.txt" && !e.staged));
    app.dispatch(Command::GitStageGroup {
        group: "Untracked".into(),
    });
    assert!(app.status.contains("No unstaged changes"));
    let view = git_ready(&mut app);
    assert!(view
        .git
        .iter()
        .any(|e| e.path == "tracked.txt" && !e.staged));
    app.dispatch(Command::GitStageGroup {
        group: "Unstaged".into(),
    });
    assert!(git_ready(&mut app).git.iter().all(|e| e.staged));
}

#[test]
fn unstage_all_handles_an_unborn_index_with_new_working_edits() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q"]);
    fs::create_dir(root.join("nested")).unwrap();
    fs::write(root.join("nested/new file.txt"), "staged\n").unwrap();
    fs::write(root.join("other.txt"), "other\n").unwrap();
    git(root, &["add", "."]);
    fs::write(root.join("nested/new file.txt"), "working\n").unwrap();
    let mut app = App::new(root).unwrap();
    git_ready(&mut app);
    app.dispatch(Command::GitUnstageAll);
    let view = git_ready(&mut app);
    assert!(view.git_error.is_empty(), "{}", view.git_error);
    assert!(view.git.iter().all(|e| e.untracked && !e.staged));
    assert_eq!(view.git.len(), 2);
    assert_eq!(
        fs::read_to_string(root.join("nested/new file.txt")).unwrap(),
        "working\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("other.txt")).unwrap(),
        "other\n"
    );
}

#[test]
fn reopening_git_and_files_reuses_the_existing_pane_tabs() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = App::new(dir.path()).unwrap();
    app.dispatch(Command::Focus { pane: 1 });
    app.dispatch(Command::AddView { kind: "git".into() });
    let count = snapshot(&mut app)
        .panes
        .iter()
        .find(|p| p.id == 1)
        .unwrap()
        .tabs
        .len();
    for kind in ["git", "files", "git", "files"] {
        app.dispatch(Command::AddView { kind: kind.into() });
        let view = snapshot(&mut app);
        let pane = view.panes.iter().find(|p| p.id == 1).unwrap();
        assert_eq!(pane.kind, kind);
        assert_eq!(pane.tabs.len(), count);
    }
}

#[test]
fn restaging_a_staged_rename_preserves_its_source_removal() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "test@example.invalid"]);
    git(root, &["config", "user.name", "Test"]);
    let base = "first unchanged line\nsecond unchanged line\nthird unchanged line\n";
    fs::write(root.join("old name.txt"), base).unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "base"]);
    git(root, &["mv", "old name.txt", "new name.txt"]);
    fs::write(root.join("new name.txt"), format!("{base}working\n")).unwrap();
    let mut app = App::new(root).unwrap();
    let view = git_ready(&mut app);
    assert!(view
        .git
        .iter()
        .any(|e| e.path == "new name.txt" && !e.staged));
    app.dispatch(Command::GitStage {
        path: "new name.txt".into(),
    });
    let view = git_ready(&mut app);
    assert!(view.git_error.is_empty(), "{}", view.git_error);
    assert!(view.git.iter().all(|e| e.staged));
    assert!(view
        .git
        .iter()
        .any(|e| e.original_path.as_deref() == Some("old name.txt")));
    let newer = format!("{base}newer working\n");
    fs::write(root.join("new name.txt"), &newer).unwrap();
    app.dispatch(Command::Refresh);
    git_ready(&mut app);
    app.dispatch(Command::GitStageGroup {
        group: "Unstaged".into(),
    });
    let view = git_ready(&mut app);
    assert!(view.git_error.is_empty(), "{}", view.git_error);
    assert!(view.git.iter().all(|e| e.staged));
    assert!(!root.join("old name.txt").exists());
    let staged = process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["show", ":new name.txt"])
        .output()
        .unwrap();
    assert_eq!(staged.stdout, newer.as_bytes());
}
