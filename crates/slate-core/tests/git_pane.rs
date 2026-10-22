//! The Git pane's history graph, branch and remote actions, discarding and
//! revealing changes, in both interfaces where they differ.
mod common;
use common::isolated;
use slate_core::{App, Command, Key};
use std::{fs, path::Path, process};

fn git(root: &Path, args: &[&str]) -> String {
    let output = process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "user.name=Slate",
            "-c",
            "user.email=slate@example.com",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}
fn commit(root: &Path, file: &str, text: &str, message: &str) {
    fs::write(root.join(file), text).unwrap();
    git(root, &["add", "--", file]);
    git(root, &["commit", "-q", "-m", message]);
}
fn repository(dir: &Path) -> std::path::PathBuf {
    let root = dir.join("repo");
    fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    root
}
/// A trusted workspace with a Git view focused in the side pane.
fn open(root: &Path, terminal: bool) -> App {
    let mut app =
        App::new_with_startup(root, Some(slate_core::preferences::StartupMode::Workspace)).unwrap();
    app.terminal_frontend = terminal;
    app.set_session_trust(true);
    app.dispatch(Command::Focus {
        pane: common::side_pane(&app),
    });
    app.dispatch(Command::AddView { kind: "git".into() });
    app.dispatch(Command::Refresh);
    app
}
fn frame(app: &mut App) -> slate_core::Snapshot {
    app.snapshot(160, 50, 1, 1, 1, 3)
}
fn git_pane(app: &mut App) -> slate_core::PaneSnapshot {
    frame(app)
        .panes
        .into_iter()
        .find(|p| p.kind == "git")
        .expect("a Git pane")
}
fn key(app: &mut App, name: &str) {
    app.dispatch(Command::Key {
        key: Key {
            key: name.into(),
            text: if name.chars().count() == 1 {
                name.into()
            } else {
                String::new()
            },
            ..Default::default()
        },
    });
}
fn action(app: &mut App, name: &str, argument: &str) {
    app.dispatch(Command::InvokeAction {
        id: name.into(),
        argument: argument.into(),
    });
}
fn idle(app: &mut App) -> bool {
    !frame(app).git_busy
}

#[test]
fn gui_diff_preference_persists_and_selects_the_viewer_only_in_the_gui() {
    let (dir, _serial) = isolated();
    let root = repository(dir.path());
    commit(&root, "a.txt", "original\n", "First");
    fs::write(root.join("a.txt"), "staged\n").unwrap();
    git(&root, &["add", "a.txt"]);
    fs::write(root.join("a.txt"), "working\n").unwrap();
    fs::write(root.join("new.txt"), "untracked\n").unwrap();

    assert_eq!(
        slate_core::preferences::Preferences::load()
            .unwrap()
            .gui_diff_viewer,
        "unified"
    );
    for (terminal, preference) in [
        (false, "unified"),
        (false, "side-by-side"),
        (true, "side-by-side"),
        (false, "unified"),
    ] {
        let mut app = open(&root, terminal);
        app.dispatch(Command::Configure {
            name: "gui-diff-viewer".into(),
            value: preference.into(),
        });
        assert_eq!(app.preferences.gui_diff_viewer, preference);
        assert_eq!(
            slate_core::preferences::Preferences::load()
                .unwrap()
                .gui_diff_viewer,
            preference
        );
        drop(app);
        // A fresh application reads the persisted choice.
        let mut app = open(&root, terminal);
        assert_eq!(app.preferences.gui_diff_viewer, preference);
        common::settle(&mut app, "staged, working and untracked changes", |a| {
            frame(a).git.len() == 3 && idle(a)
        });
        for (staged, path, before, after) in [
            (true, "a.txt", "original", "staged"),
            (false, "a.txt", "staged", "working"),
            (false, "new.txt", "", "untracked"),
        ] {
            let row = frame(&mut app)
                .git
                .iter()
                .position(|e| e.path == path && e.staged == staged)
                .unwrap();
            let pane = git_pane(&mut app).id;
            app.dispatch(Command::Click {
                pane,
                row,
                col: 0,
                shift: false,
            });
            let existing = app.documents.len();
            action(&mut app, "diff", "");
            common::settle(&mut app, "preferred diff viewer", |a| {
                a.documents.len() > existing && idle(a)
            });
            let split = !terminal && preference == "side-by-side";
            let suffix = format!(" · {path}");
            let docs: Vec<_> = app
                .documents
                .values()
                .filter(|d| d.label.as_ref().is_some_and(|l| l.ends_with(&suffix)))
                .collect();
            // a.txt has separate staged and working previews.
            let title = if split {
                if staged {
                    "HEAD → index"
                } else {
                    "index → working copy"
                }
            } else if staged {
                "Staged diff"
            } else {
                "Working diff"
            };
            let docs: Vec<_> = docs
                .into_iter()
                .filter(|d| d.label.as_ref().unwrap().contains(title))
                .collect();
            assert_eq!(docs.len(), if split { 2 } else { 1 });
            assert!(docs.iter().all(|d| d.read_only));
            if split {
                let left = docs
                    .iter()
                    .find(|d| d.label.as_ref().unwrap().starts_with("Before"))
                    .unwrap();
                let right = docs
                    .iter()
                    .find(|d| d.label.as_ref().unwrap().starts_with("After"))
                    .unwrap();
                assert!(left.text().contains(before));
                assert!(right.text().contains(after));
                let visible: Vec<_> = frame(&mut app)
                    .panes
                    .into_iter()
                    .filter(|p| p.kind == "editor")
                    .collect();
                assert!(visible.len() >= 2);
            } else {
                assert!(docs[0].text().contains(before));
                assert!(docs[0].text().contains(after));
            }
        }
    }
}

#[test]
fn invalid_gui_diff_preference_keeps_the_saved_choice() {
    let (dir, _serial) = isolated();
    let root = repository(dir.path());
    let mut app = open(&root, false);
    app.dispatch(Command::Configure {
        name: "gui-diff-viewer".into(),
        value: "side-by-side".into(),
    });
    let path = slate_core::preferences::Preferences::path();
    let saved = fs::read(&path).unwrap();
    app.dispatch(Command::Configure {
        name: "gui-diff-viewer".into(),
        value: "invalid".into(),
    });
    assert!(
        app.status.contains("gui_diff_viewer must be"),
        "{}",
        app.status
    );
    assert_eq!(app.preferences.gui_diff_viewer, "side-by-side");
    assert_eq!(fs::read(path).unwrap(), saved);
}

#[test]
fn the_graph_follows_the_changes_and_opens_commits() {
    let (dir, _serial) = isolated();
    let root = repository(dir.path());
    commit(&root, "a.txt", "a\n", "First");
    let branch = git(&root, &["symbolic-ref", "--short", "HEAD"]);
    git(&root, &["switch", "-q", "-c", "feature"]);
    commit(&root, "b.txt", "b\n", "On the branch");
    git(&root, &["switch", "-q", &branch]);
    commit(&root, "c.txt", "c\n", "On the trunk");
    git(
        &root,
        &["merge", "-q", "--no-ff", "feature", "-m", "Merge feature"],
    );
    git(&root, &["tag", "v1"]);
    fs::write(root.join("a.txt"), "edited\n").unwrap();

    let mut app = open(&root, false);
    common::settle(&mut app, "history and changes", |a| {
        let f = frame(a);
        f.history.len() == 4 && f.git.len() == 1 && !f.git_busy
    });
    let f = frame(&mut app);
    let merge = &f.history[0];
    assert_eq!(merge.subject, "Merge feature");
    assert!(merge.head && merge.parents.len() == 2 && merge.lanes == 2);
    assert!(merge
        .refs
        .iter()
        .any(|r| r.kind == "head" && r.name == branch));
    assert!(merge.refs.iter().any(|r| r.kind == "tag" && r.name == "v1"));
    assert!(f.history.iter().any(|c| c
        .refs
        .iter()
        .any(|r| r.name == "feature" && r.kind == "branch")));
    assert_eq!(f.history[3].subject, "First");
    assert!(!f.history_more && f.git_branches.contains(&"feature".to_string()));

    // The selection continues from the changes into the graph.
    assert_eq!(git_pane(&mut app).selected, 0);
    key(&mut app, "Down");
    key(&mut app, "Down");
    assert_eq!(git_pane(&mut app).selected, 2);
    let selected_hash = f.history[1].hash.clone();
    // Stage-only actions do not apply to a commit.
    let pane = git_pane(&mut app).id;
    let stage = app.command_catalog_for("stage", pane, None);
    assert!(stage.iter().any(|c| c.id == "stage" && !c.enabled));

    // A new change above keeps the same commit selected.
    fs::write(root.join("b.txt"), "edited\n").unwrap();
    app.dispatch(Command::Refresh);
    common::settle(&mut app, "second change", |a| {
        frame(a).git.len() == 2 && idle(a)
    });
    let f = frame(&mut app);
    let pane = git_pane(&mut app);
    assert_eq!(pane.selected, 3);
    assert_eq!(f.history[pane.selected - f.git.len()].hash, selected_hash);

    // Enter opens the commit read-only, in one reused document.
    let subject = f.history[pane.selected - f.git.len()].subject.clone();
    key(&mut app, "Enter");
    common::settle(&mut app, "commit document", |a| {
        a.documents.values().any(|d| {
            d.label.as_deref().is_some_and(|l| l.ends_with(" · commit"))
                && d.text().contains(&subject)
        })
    });
    action(&mut app, "git-show", &f.history[3].hash);
    common::settle(&mut app, "second commit document", |a| {
        a.documents.values().any(|d| d.text().contains("First"))
    });
    let commits = app
        .documents
        .values()
        .filter(|d| d.label.as_deref().is_some_and(|l| l.ends_with(" · commit")))
        .collect::<Vec<_>>();
    assert_eq!(commits.len(), 1, "commit details reuse one document");
    assert!(commits[0].read_only);
    // Only object names are accepted.
    action(&mut app, "git-show", "--output=/tmp/x");
    assert!(app.status.contains("Select a commit"), "{}", app.status);
}

#[test]
fn discarding_restores_files_and_reveal_selects_them_in_both_interfaces() {
    for terminal in [false, true] {
        let (dir, _serial) = isolated();
        let root = repository(dir.path());
        fs::create_dir_all(root.join("nested/deeper")).unwrap();
        commit(&root, "nested/deeper/kept.txt", "original\n", "First");
        commit(&root, "staged.txt", "original\n", "Second");
        fs::write(root.join("nested/deeper/kept.txt"), "changed\n").unwrap();
        fs::write(root.join("staged.txt"), "changed\n").unwrap();
        git(&root, &["add", "staged.txt"]);

        let mut app = open(&root, terminal);
        common::settle(&mut app, "changes", |a| frame(a).git.len() == 2 && idle(a));
        let rows = frame(&mut app).git;
        let staged = rows.iter().position(|e| e.staged).unwrap();
        let working = rows.iter().position(|e| !e.staged).unwrap();
        let pane = git_pane(&mut app).id;
        let discard = |app: &App, row| {
            app.command_catalog_for("discard-changes", pane, Some(row))
                .into_iter()
                .find(|c| c.id == "discard-changes")
                .unwrap()
                .enabled
        };
        assert!(!discard(&app, staged), "staged changes are not discarded");
        assert!(discard(&app, working));

        // Reveal selects the file in the file browser.
        app.dispatch(Command::Click {
            pane,
            row: working,
            col: 0,
            shift: false,
        });
        action(&mut app, "reveal-in-files", "");
        let target = root.canonicalize().unwrap().join("nested/deeper/kept.txt");
        common::settle(&mut app, "revealed file", |a| {
            let f = frame(a);
            let files = f
                .panes
                .iter()
                .find(|p| p.id == f.focus && p.kind == "files");
            files.is_some_and(|p| {
                f.files
                    .get(p.selected)
                    .is_some_and(|e| Path::new(&e.path) == target)
            })
        });

        // Discarding asks first; N keeps the change, Y restores the file.
        // Revealing showed the Files tab of the shared side pane.
        app.dispatch(Command::Focus { pane });
        app.dispatch(Command::AddView { kind: "git".into() });
        // Revealing refreshed Git; discarding waits for running operations.
        common::settle(&mut app, "idle Git", idle);
        app.dispatch(Command::Click {
            pane,
            row: working,
            col: 0,
            shift: false,
        });
        key(&mut app, "d");
        assert_eq!(app.prompt.as_ref().unwrap().kind, "discard-changes");
        key(&mut app, "n");
        assert!(app.prompt.is_none());
        assert_eq!(fs::read_to_string(&target).unwrap(), "changed\n");
        key(&mut app, "d");
        key(&mut app, "d");
        assert!(app.prompt.is_some(), "a repeated D does not confirm");
        key(&mut app, "y");
        common::settle(&mut app, "restored file", |a| {
            frame(a).git.len() == 1 && idle(a)
        });
        assert_eq!(fs::read_to_string(&target).unwrap(), "original\n");
        assert_eq!(
            fs::read_to_string(root.join("staged.txt")).unwrap(),
            "changed\n"
        );
    }
}

#[test]
fn branches_switch_and_sync_with_a_remote_without_prompting() {
    let (dir, _serial) = isolated();
    let remote = dir.path().join("remote.git");
    process::Command::new("git")
        .args(["init", "-q", "--bare"])
        .arg(&remote)
        .status()
        .unwrap();
    let root = repository(dir.path());
    commit(&root, "a.txt", "a\n", "First");
    git(
        &root,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    let branch = git(&root, &["symbolic-ref", "--short", "HEAD"]);

    let mut app = open(&root, false);
    common::settle(&mut app, "repository", |a| {
        frame(a).history.len() == 1 && idle(a)
    });
    assert!(frame(&mut app).git_upstream.is_empty());
    // A branch without an upstream is pushed to its only remote, tracking it.
    action(&mut app, "git-push", "");
    common::settle(&mut app, "first push", |a| {
        !frame(a).git_upstream.is_empty() && idle(a)
    });
    assert_eq!(frame(&mut app).git_upstream, format!("origin/{branch}"));

    commit(&root, "b.txt", "b\n", "Second");
    app.dispatch(Command::Refresh);
    common::settle(&mut app, "ahead", |a| frame(a).git_ahead == 1 && idle(a));
    action(&mut app, "git-push", "");
    common::settle(&mut app, "pushed", |a| frame(a).git_ahead == 0 && idle(a));

    // Another clone pushes; fetch shows it, pull fast-forwards to it.
    let other = dir.path().join("other");
    process::Command::new("git")
        .args(["clone", "-q", "--branch", &branch])
        .arg(&remote)
        .arg(&other)
        .status()
        .unwrap();
    commit(&other, "c.txt", "c\n", "From elsewhere");
    git(&other, &["push", "-q"]);
    action(&mut app, "git-fetch", "");
    common::settle(&mut app, "behind", |a| frame(a).git_behind == 1 && idle(a));
    // The graph also shows the upstream's newer commit.
    common::settle(&mut app, "upstream in graph", |a| {
        frame(a)
            .history
            .iter()
            .any(|c| c.subject == "From elsewhere")
    });
    action(&mut app, "git-pull", "");
    common::settle(&mut app, "pulled", |a| {
        let f = frame(a);
        f.git_behind == 0
            && !f.git_busy
            && f.history
                .first()
                .is_some_and(|c| c.head && c.subject == "From elsewhere")
    });
    assert!(root.join("c.txt").exists());

    // Create and switch branches; names that look like options are refused.
    action(&mut app, "git-branch", "topic");
    common::settle(&mut app, "new branch", |a| {
        frame(a).git_branch == "topic" && idle(a)
    });
    action(&mut app, "git-switch", &branch);
    common::settle(&mut app, "switched back", |a| {
        frame(a).git_branch == branch && idle(a)
    });
    assert!(frame(&mut app).git_branches.contains(&"topic".to_string()));
    action(&mut app, "git-switch", "--orphan");
    assert!(app.status.contains("Not a branch name"), "{}", app.status);
    // Without an argument, switching asks for the branch.
    action(&mut app, "git-switch", "");
    assert_eq!(app.prompt.as_ref().unwrap().kind, "git-switch");
}

fn split_diff(app: &mut App) -> Vec<slate_core::PaneSnapshot> {
    common::settle(app, "Git ready for comparison", |a| {
        !frame(a).git.is_empty() && idle(a)
    });
    let pane = git_pane(app).id;
    app.dispatch(Command::Focus { pane });
    common::settle(app, "focused Git ready", idle);
    let count = app.documents.len();
    action(app, "diff-side-by-side", "");
    common::settle(app, "split comparison", |a| {
        a.documents.len() == count + 2 && idle(a)
    });
    frame(app)
        .panes
        .into_iter()
        .filter(|p| {
            p.editor
                .as_ref()
                .is_some_and(|e| app.documents[&e.document].label.is_some())
        })
        .collect()
}

#[test]
fn split_comparisons_restore_layout_and_link_both_scroll_axes() {
    let (dir, _serial) = isolated();
    let root = repository(dir.path());
    let before: String = (0..180)
        .map(|i| format!("old {i} {}\n", "x".repeat(400)))
        .collect();
    let after: String = (0..180).map(|i| format!("new {i}\n")).collect();
    commit(&root, "a.txt", &before, "First");
    fs::write(root.join("a.txt"), after).unwrap();
    let mut app = open(&root, false);
    common::settle(&mut app, "working diff", |a| {
        frame(a).git.len() == 1 && idle(a)
    });
    app.dispatch(Command::Configure {
        name: "soft-wrap".into(),
        value: "true".into(),
    });
    let original = serde_json::to_value(&app.layout).unwrap();
    let original_focus = app.focus;
    let original_docs = app.documents.len();
    for close_left in [false, true, false] {
        let panes = split_diff(&mut app);
        assert_eq!(panes.len(), 2);
        assert_eq!(app.layout.panes().len(), 4);
        assert!(panes.iter().all(|p| !p.editor.as_ref().unwrap().soft_wrap));
        let left = panes[0].id;
        let right = panes[1].id;
        // Wheel, vertical scrollbar and horizontal scrolling all work from
        // either side, even when only the opposite side contains long lines.
        for (pane, line, column) in [(left, 40, 60), (right, 80, 35)] {
            app.dispatch(Command::ScrollTo { pane, line });
            app.dispatch(Command::ScrollColumns {
                pane,
                delta: column,
            });
            let pair: Vec<_> = frame(&mut app)
                .panes
                .into_iter()
                .filter(|p| p.id == left || p.id == right)
                .collect();
            let a = pair[0].editor.as_ref().unwrap();
            let b = pair[1].editor.as_ref().unwrap();
            assert_eq!(a.top, line);
            assert_eq!((a.top, a.left), (b.top, b.left));
            assert!(a.left > 0);
        }
        app.dispatch(Command::Scroll {
            pane: right,
            delta: -7,
        });
        let pair = frame(&mut app);
        let a = pair
            .panes
            .iter()
            .find(|p| p.id == left)
            .unwrap()
            .editor
            .as_ref()
            .unwrap();
        let b = pair
            .panes
            .iter()
            .find(|p| p.id == right)
            .unwrap()
            .editor
            .as_ref()
            .unwrap();
        assert_eq!(a.top, 87);
        assert_eq!(a.top, b.top);
        // Navigation from the right pane must synchronize the left pane in
        // this frame, although the left pane is rendered first.
        app.dispatch(Command::Focus { pane: right });
        app.dispatch(Command::Key {
            key: Key {
                key: "End".into(),
                ctrl: true,
                ..Default::default()
            },
        });
        let pair = frame(&mut app);
        let a = pair
            .panes
            .iter()
            .find(|p| p.id == left)
            .unwrap()
            .editor
            .as_ref()
            .unwrap();
        let b = pair
            .panes
            .iter()
            .find(|p| p.id == right)
            .unwrap()
            .editor
            .as_ref()
            .unwrap();
        assert_eq!((a.top, a.left), (b.top, b.left));
        assert!(a.top > 87);
        let pane = if close_left { left } else { right };
        let slate_core::layout::View::Editor(view) = app.layout.view(pane).unwrap() else {
            panic!("editor")
        };
        app.dispatch(Command::CloseTab {
            pane,
            view: *view,
            force: false,
        });
        assert_eq!(serde_json::to_value(&app.layout).unwrap(), original);
        assert_eq!(app.focus, original_focus);
        assert_eq!(app.documents.len(), original_docs);
    }
}

#[test]
fn nested_comparisons_close_in_either_order_and_keep_new_files() {
    let (dir, _serial) = isolated();
    let root = repository(dir.path());
    commit(&root, "a.txt", "old\n", "First");
    fs::write(root.join("a.txt"), "new\n").unwrap();
    let mut app = open(&root, false);
    common::settle(&mut app, "working diff", |a| {
        frame(a).git.len() == 1 && idle(a)
    });
    let original = serde_json::to_value(&app.layout).unwrap();
    for oldest_first in [true, false] {
        let first = split_diff(&mut app);
        let first_view = app.layout.view(first[0].id).unwrap().clone();
        let second = split_diff(&mut app);
        let second_view = app
            .layout
            .view(
                second
                    .iter()
                    .max_by_key(|p| p.editor.as_ref().unwrap().document)
                    .unwrap()
                    .id,
            )
            .unwrap()
            .clone();
        let mut close = |view: &slate_core::layout::View| {
            let pane = app
                .layout
                .panes()
                .into_iter()
                .find(|p| app.layout.tabs(*p).unwrap().contains(view))
                .unwrap();
            let slate_core::layout::View::Editor(view) = view else {
                panic!("editor")
            };
            app.dispatch(Command::CloseTab {
                pane,
                view: *view,
                force: false,
            });
        };
        if oldest_first {
            close(&first_view);
            close(&second_view);
        } else {
            close(&second_view);
            close(&first_view);
        }
        assert_eq!(serde_json::to_value(&app.layout).unwrap(), original);
    }
    let panes = split_diff(&mut app);
    let temporary = panes.last().unwrap().id;
    app.dispatch(Command::Focus { pane: temporary });
    app.dispatch(Command::Open {
        path: root.join("a.txt"),
    });
    common::settle(&mut app, "file opened in comparison pane", |a| {
        a.layout.view(temporary).is_some_and(|v| match v {
            slate_core::layout::View::Editor(id) => {
                a.documents[&a.views[id].document].path.as_deref()
                    == Some(root.join("a.txt").as_path())
            }
            _ => false,
        })
    });
    let slate_core::layout::View::Editor(file_view) = app.layout.view(temporary).unwrap() else {
        panic!("file editor")
    };
    let doc = app.views[file_view].document;
    let other = panes[0].id;
    app.dispatch(Command::Focus { pane: other });
    app.dispatch(Command::ClosePane);
    assert!(app.layout.tabs(temporary).is_some());
    assert!(app.documents.contains_key(&doc));
    assert!(app
        .documents
        .values()
        .any(|d| d.path.as_deref() == Some(root.join("a.txt").as_path())));
    assert!(!app.documents.values().any(|d| d
        .label
        .as_ref()
        .is_some_and(|l| l.starts_with("Before") || l.starts_with("After"))));
}

#[test]
fn temporary_diff_splits_are_not_recovered_and_failed_opens_are_atomic() {
    let (dir, _serial) = isolated();
    let root = repository(dir.path());
    commit(&root, "a.txt", "old\n", "First");
    fs::write(root.join("a.txt"), "new\n").unwrap();
    let mut app = open(&root, false);
    common::settle(&mut app, "working diff", |a| {
        frame(a).git.len() == 1 && idle(a)
    });
    app.enable_workspace(false).unwrap();
    let original = serde_json::to_value(&app.layout).unwrap();
    split_diff(&mut app);
    app.flush_workspace().unwrap();
    drop(app);
    let mut app = open(&root, false);
    app.enable_workspace(true).unwrap();
    assert_eq!(serde_json::to_value(&app.layout).unwrap(), original);
    assert!(!app.documents.values().any(|d| d.label.is_some()));
    common::settle(&mut app, "working diff", |a| {
        frame(a).git.len() == 1 && idle(a)
    });
    let pane = common::editor_pane(&app);
    // Saturate the editor branch to the supported nesting limit.
    for id in 10000..10020 {
        if app.layout.depth_of(pane).unwrap() >= slate_core::layout::MAX_DEPTH {
            break;
        }
        app.layout.split(
            pane,
            slate_core::layout::Axis::Horizontal,
            id * 2,
            id * 2 + 1,
            slate_core::layout::View::Files,
        );
    }
    let git = git_pane(&mut app).id;
    app.dispatch(Command::Focus { pane: git });
    let original = serde_json::to_value(&app.layout).unwrap();
    let docs = app.documents.len();
    let views = app.views.len();
    action(&mut app, "diff-side-by-side", "");
    common::settle(&mut app, "rejected saturated comparison", idle);
    assert!(app.status.contains("Not enough room"), "{}", app.status);
    assert_eq!(serde_json::to_value(&app.layout).unwrap(), original);
    assert_eq!(app.documents.len(), docs);
    assert_eq!(app.views.len(), views);
}
