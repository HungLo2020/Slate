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
