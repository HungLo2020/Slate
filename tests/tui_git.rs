//! The Git pane in the terminal interface, driven through a real
//! pseudo-terminal: changes followed by the commit graph, drawn with
//! box-drawing lanes, and commits opened read-only.
mod support;
use std::{fs, path::Path, process};
use support::Env;

fn git(root: &Path, args: &[&str]) {
    let status = process::Command::new("git")
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
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

#[test]
fn the_git_pane_lists_changes_then_a_drawn_commit_graph() {
    let env = Env::new();
    let root = env.path("repo");
    fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "-q"]);
    let commit = |file: &str, message: &str| {
        fs::write(root.join(file), message).unwrap();
        git(&root, &["add", "--", file]);
        git(&root, &["commit", "-q", "-m", message]);
    };
    commit("a.txt", "First commit");
    git(&root, &["switch", "-q", "-c", "feature"]);
    commit("b.txt", "Branch work");
    git(&root, &["switch", "-q", "main"]);
    commit("c.txt", "Trunk work");
    git(
        &root,
        &["merge", "-q", "--no-ff", "feature", "-m", "Merge feature"],
    );
    git(&root, &["tag", "v1"]);
    fs::write(root.join("a.txt"), "edited").unwrap();
    // Git runs only in trusted folders.
    let config = env.path("config/slate");
    fs::create_dir_all(&config).unwrap();
    fs::write(
        config.join("trusted-folders"),
        format!("{}\n", root.canonicalize().unwrap().display()),
    )
    .unwrap();

    let mut tui = env.slate(&[root.to_str().unwrap()]);
    tui.wait_for("files #1");
    tui.command("git");
    tui.wait_for("Git: main");
    tui.wait_for("[Unstaged]");
    // The merge forks into a second lane; its branch and tag are labelled.
    tui.wait_for("●─╮");
    tui.wait_for("Merge feature (main, v1)");
    tui.wait_for("First commit");
    // Moving past the change selects commits; Enter opens one read-only.
    tui.send(b"\x1b[B\r");
    tui.wait_for("· commit");
    tui.wait_for("Author:");
}
