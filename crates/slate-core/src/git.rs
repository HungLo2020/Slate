//! Git integration. Commands run with a timeout so a hung credential helper
//! or a huge repository cannot wedge the Git pane, paths keep their raw
//! bytes so files with non-UTF-8 names can still be staged, and untrusted
//! workspaces never run repository-configured programs (fsmonitor, hooks).
use serde::Serialize;
use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::{Command, Output},
    time::Duration,
};

/// Reads (status, diff) and writes (stage, commit) have separate budgets.
pub const READ_TIMEOUT: Duration = Duration::from_secs(30);
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Serialize, PartialEq)]
pub struct GitEntry {
    pub status: String,
    pub path: String,
    pub original_path: Option<String>,
    pub staged: bool,
    pub untracked: bool,
    pub group: String,
    /// The path exactly as Git reported it.
    #[serde(skip)]
    pub raw_path: OsString,
    #[serde(skip)]
    pub raw_original: Option<OsString>,
}
impl GitEntry {
    pub(crate) fn staging_paths(&self) -> impl Iterator<Item = OsString> {
        // An index rename already removed the source. Passing that missing
        // source to `git add` rejects a later edit to the destination. Include
        // both paths only when the rename still belongs to the working tree.
        let source = self.raw_original.clone().filter(|_| {
            self.status.as_bytes().get(1) == Some(&b'R') && !self.status.starts_with('R')
        });
        std::iter::once(self.raw_path.clone()).chain(source)
    }
}
#[derive(Default)]
pub struct GitState {
    pub entries: Vec<GitEntry>,
    pub branch: String,
    pub repository: bool,
    /// The checked-out commit; empty before the first commit.
    pub head: String,
    /// The current branch's upstream (`origin/main`) and its commit, and how
    /// far the branch is ahead of and behind it. Empty without an upstream.
    pub upstream: String,
    pub upstream_head: String,
    pub ahead: usize,
    pub behind: usize,
    /// Local branches, most recently committed first.
    pub branches: Vec<String>,
}

/// The Git pane's state.
pub(crate) struct Panel {
    pub entries: Vec<GitEntry>,
    pub selected: usize,
    pub branch: String,
    pub repository: bool,
    /// Requests whose replies are outstanding.
    pub jobs: usize,
    pub error: String,
    /// Bumped when the entries change, for frontends.
    pub revision: u64,
    /// A status read is running; another was requested meanwhile.
    pub status_running: bool,
    pub status_again: bool,
    /// The repository is in a folder the user has not trusted, so Git (which
    /// can run programs named by repository configuration) does not run.
    pub restricted: bool,
    /// The repository's top folder, which status paths are relative to;
    /// empty outside a repository.
    pub top: String,
    pub head: String,
    pub upstream: String,
    pub upstream_head: String,
    pub ahead: usize,
    pub behind: usize,
    pub branches: Vec<String>,
    /// The commit graph, listed after `entries`: a `selected` index past the
    /// last entry selects `history[selected - entries.len()]`.
    pub history: Vec<crate::git_history::HistoryRow>,
    pub history_revision: u64,
    /// Commits requested; "load more" raises it a page at a time.
    pub history_limit: usize,
    pub history_running: bool,
    pub history_again: bool,
    /// The (HEAD, upstream) commits the history was read for, and whether a
    /// refresh asked for it to be read again regardless.
    pub history_key: (String, String),
    pub history_stale: bool,
    pub history_error: String,
}
impl Default for Panel {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            selected: 0,
            branch: String::new(),
            repository: false,
            jobs: 0,
            error: String::new(),
            revision: 1,
            status_running: false,
            status_again: false,
            restricted: false,
            top: String::new(),
            head: String::new(),
            upstream: String::new(),
            upstream_head: String::new(),
            ahead: 0,
            behind: 0,
            branches: Vec::new(),
            history: Vec::new(),
            history_revision: 1,
            history_limit: crate::git_history::PAGE,
            history_running: false,
            history_again: false,
            history_key: Default::default(),
            history_stale: false,
            history_error: String::new(),
        }
    }
}

/// Where to run Git: the repository's top folder, and the workspace's path
/// inside it when the repository is larger than the workspace (status and
/// "stage all" stay within the workspace).
#[derive(Clone)]
pub struct Context {
    pub root: PathBuf,
    pub scope: Option<OsString>,
}

/// The repository containing `folder`: the nearest folder with a `.git`
/// entry. Found without running Git, so nothing executes before trust.
pub fn find_repository(folder: &Path) -> Option<PathBuf> {
    let folder = folder
        .canonicalize()
        .unwrap_or_else(|_| folder.to_path_buf());
    folder
        .ancestors()
        .find(|d| d.join(".git").exists())
        .map(Path::to_path_buf)
}

pub enum GitJob {
    Status(Context),
    Diff {
        context: Context,
        path: OsString,
        display: String,
        staged: bool,
        untracked: bool,
    },
    Run(Context, Vec<OsString>),
    Comparison {
        context: Context,
        path: OsString,
        display: String,
        staged: bool,
        untracked: bool,
    },
    Hunk(std::sync::Arc<crate::comparison::Preview>, usize),
    /// Read up to `limit` commits of HEAD and its upstream for the graph.
    History {
        context: Context,
        limit: usize,
        head: String,
    },
    /// A commit's message, statistics and patch, for a read-only document.
    Show {
        context: Context,
        hash: String,
    },
    /// Talk to the branch's remote. Never prompts for credentials.
    Remote(Context, Remote),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Remote {
    Fetch,
    /// Fast-forward only: a pull never creates a merge or stops on conflicts.
    Pull,
    /// Push, setting an upstream on the only (or `origin`) remote when the
    /// branch has none yet.
    Push,
}

pub enum GitReply {
    Status(Result<GitState, String>),
    Operation(Result<String, String>),
    Preview(Result<crate::comparison::Preview, String>),
    History(Result<Vec<crate::git_history::HistoryRow>, String>),
    Show(String, Result<String, String>),
}

#[cfg(unix)]
fn os(bytes: &[u8]) -> OsString {
    use std::os::unix::ffi::OsStrExt;
    OsStr::from_bytes(bytes).to_os_string()
}
#[cfg(not(unix))]
fn os(bytes: &[u8]) -> OsString {
    String::from_utf8_lossy(bytes).into_owned().into()
}

/// Git for the repository at `root`: never prompting, never taking the
/// index lock for optional work, and blind to any repository a parent Git
/// described in the environment (Slate as the commit editor), so `-C root`
/// alone decides the repository.
pub(crate) fn command(root: &Path) -> Command {
    let mut command = Command::new("git");
    crate::process::scrub_repository_env(&mut command)
        .arg("-C")
        .arg(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        // Status must not take the index lock away from the user's own Git.
        .env("GIT_OPTIONAL_LOCKS", "0");
    command
}

/// Run git in the repository, killing it (and anything it started) after
/// `timeout`. Only called for trusted repositories.
pub fn run(context: &Context, args: &[OsString], timeout: Duration) -> Result<Output, String> {
    let mut command = command(&context.root);
    command.args(args);
    let label = format!("git {}", subcommand(args));
    crate::process::run(&mut command, None, timeout, &label)
}

fn args(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

pub fn handle(job: GitJob) -> GitReply {
    match job {
        GitJob::Status(context) => GitReply::Status(status(&context)),
        GitJob::Diff {
            context,
            path,
            display,
            staged,
            untracked,
        } => preview(context, path, display, staged, untracked, false),
        GitJob::Comparison {
            context,
            path,
            display,
            staged,
            untracked,
        } => preview(context, path, display, staged, untracked, true),
        GitJob::Hunk(preview, index) => {
            GitReply::Operation(crate::comparison::apply_hunk(&preview, index))
        }
        GitJob::Run(context, args) => GitReply::Operation(operation(&context, args)),
        GitJob::History {
            context,
            limit,
            head,
        } => GitReply::History(history(&context, limit, &head)),
        GitJob::Show { context, hash } => {
            let result = show(&context, &hash);
            GitReply::Show(hash, result)
        }
        GitJob::Remote(context, remote) => GitReply::Operation(remote_operation(&context, remote)),
    }
}

/// The successful, trimmed output of a read, or empty.
fn read_text(context: &Context, list: &[&str]) -> String {
    run(context, &args(list), READ_TIMEOUT)
        .ok()
        .filter(|r| r.status.success())
        .map(|r| String::from_utf8_lossy(&r.stdout).trim().to_string())
        .unwrap_or_default()
}

fn history(
    context: &Context,
    limit: usize,
    head: &str,
) -> Result<Vec<crate::git_history::HistoryRow>, String> {
    if head.is_empty() {
        // Nothing is committed yet.
        return Ok(Vec::new());
    }
    let mut list = args(&["log", "--topo-order", "--decorate=full", "--no-color"]);
    list.push(crate::git_history::LOG_FORMAT.into());
    list.push(format!("--max-count={}", limit.clamp(1, crate::git_history::MAX)).into());
    list.push("HEAD".into());
    if !read_text(context, &["rev-parse", "--verify", "-q", "@{upstream}"]).is_empty() {
        list.push("@{upstream}".into());
    }
    list.push("--".into());
    let result = run(context, &list, READ_TIMEOUT)?;
    if !result.status.success() {
        return Err(String::from_utf8_lossy(&result.stderr).trim().to_string());
    }
    Ok(crate::git_history::layout(
        crate::git_history::parse_log(&result.stdout),
        head,
    ))
}

/// Hex object names only, so the argument can never be read as an option.
pub(crate) fn valid_hash(hash: &str) -> bool {
    (4..=64).contains(&hash.len()) && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

fn show(context: &Context, hash: &str) -> Result<String, String> {
    if !valid_hash(hash) {
        return Err("Not a commit".into());
    }
    let mut list = args(&[
        // File names as they are, not octal-escaped.
        "-c",
        "core.quotePath=false",
        "show",
        "--stat",
        "--patch",
        "--format=fuller",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
    ]);
    list.push(hash.into());
    list.push("--".into());
    let result = run(context, &list, READ_TIMEOUT)?;
    if !result.status.success() {
        return Err(String::from_utf8_lossy(&result.stderr).trim().to_string());
    }
    let mut text = String::from_utf8_lossy(&result.stdout).into_owned();
    if text.len() > 2 * 1024 * 1024 {
        let mut end = 2 * 1024 * 1024;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n[Commit preview truncated at 2 MiB]\n");
    }
    Ok(text)
}

/// Run a command that may contact a remote. Git never prompts (see
/// [`command`]); SSH is kept from prompting too, unless the user configured
/// their own SSH command, because Slate's process group cannot read the
/// terminal and a prompt would wait until the timeout.
fn run_remote(context: &Context, list: &[OsString]) -> Result<Output, String> {
    let mut command = command(&context.root);
    let configured = std::env::var_os("GIT_SSH_COMMAND").is_some()
        || std::env::var_os("GIT_SSH").is_some()
        || !read_text(context, &["config", "--get", "core.sshCommand"]).is_empty();
    if !configured {
        command.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    command.args(list);
    let label = format!("git {}", subcommand(list));
    crate::process::run(&mut command, None, WRITE_TIMEOUT, &label)
}

/// The Git subcommand in an argument list, past any `-c name=value` options.
fn subcommand(list: &[OsString]) -> String {
    let mut args = list.iter();
    while let Some(arg) = args.next() {
        if arg == "-c" {
            args.next();
            continue;
        }
        return arg.to_string_lossy().into_owned();
    }
    String::new()
}

fn remote_operation(context: &Context, remote: Remote) -> Result<String, String> {
    let list = match remote {
        Remote::Fetch => args(&["fetch"]),
        Remote::Pull => args(&["pull", "--ff-only"]),
        Remote::Push => {
            let upstream = read_text(
                context,
                &[
                    "rev-parse",
                    "--abbrev-ref",
                    "--symbolic-full-name",
                    "@{upstream}",
                ],
            );
            if upstream.is_empty() {
                let remotes = read_text(context, &["remote"]);
                let remotes: Vec<&str> = remotes.lines().collect();
                let target = if remotes.contains(&"origin") {
                    "origin"
                } else if let [only] = remotes[..] {
                    only
                } else if remotes.is_empty() {
                    return Err("This repository has no remote to push to".into());
                } else {
                    return Err("Choose a remote: set this branch's upstream first".into());
                };
                let mut list = args(&["push", "--set-upstream"]);
                list.push(target.into());
                list.push("HEAD".into());
                list
            } else {
                args(&["push"])
            }
        }
    };
    let result = run_remote(context, &list)?;
    // Fetch, pull and push report progress on standard error.
    let output = |bytes: &[u8]| String::from_utf8_lossy(bytes).trim().to_string();
    if result.status.success() {
        let message = output(&result.stdout);
        Ok(if message.is_empty() {
            match remote {
                Remote::Fetch => "Fetched".into(),
                Remote::Pull => "Pulled".into(),
                Remote::Push => "Pushed".into(),
            }
        } else {
            message
        })
    } else {
        let error = output(&result.stderr);
        Err(if error.is_empty() {
            output(&result.stdout)
        } else {
            error
        })
    }
}

fn operation(context: &Context, mut list: Vec<OsString>) -> Result<String, String> {
    if list.first().is_some_and(|a| a == "reset")
        && !run(
            context,
            &args(&["rev-parse", "--verify", "HEAD"]),
            READ_TIMEOUT,
        )
        .is_ok_and(|r| r.status.success())
    {
        let paths = list
            .iter()
            .position(|a| a == "--")
            .map(|i| list[i + 1..].to_vec())
            .unwrap_or_default();
        // An unborn index has no HEAD to reset to. Remove its entries only,
        // including nested paths and entries whose working copy has changed.
        list = args(&["rm", "--cached", "-r", "--force", "--"]);
        list.extend(paths);
    }
    match run(context, &list, WRITE_TIMEOUT)? {
        result if result.status.success() => {
            Ok(String::from_utf8_lossy(&result.stdout).trim().to_string())
        }
        result => Err(String::from_utf8_lossy(&result.stderr).trim().to_string()),
    }
}

fn preview(
    context: Context,
    path: OsString,
    display: String,
    staged: bool,
    untracked: bool,
    side_by_side: bool,
) -> GitReply {
    let result =
        load_patch(&context, &path, staged, untracked).map(|patch| crate::comparison::Preview {
            context,
            path,
            display,
            staged,
            untracked,
            patch,
            side_by_side,
        });
    GitReply::Preview(result)
}
pub(crate) fn load_patch(
    context: &Context,
    path: &std::ffi::OsStr,
    staged: bool,
    untracked: bool,
) -> Result<String, String> {
    let mut list = args(&["diff", "--no-ext-diff", "--no-textconv", "--no-color"]);
    if untracked {
        list.extend(args(&["--no-index", "--", "/dev/null"]));
    } else {
        if staged {
            list.push("--cached".into());
        }
        list.push("--".into());
    }
    list.push(path.to_owned());
    let result = run(context, &list, READ_TIMEOUT)?;
    if !result.status.success() && !(untracked && result.status.code() == Some(1)) {
        return Err(String::from_utf8_lossy(&result.stderr).trim().into());
    }
    let mut text = String::from_utf8(result.stdout).map_err(|_| "Diff is not UTF-8".to_string())?;
    if text.len() > 2 * 1024 * 1024 {
        let mut end = 2 * 1024 * 1024;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n[Diff preview truncated at 2 MiB]\n");
    }
    Ok(text)
}

fn status(context: &Context) -> Result<GitState, String> {
    let mut list = args(&["status", "--porcelain=v1", "--untracked-files=all", "-z"]);
    if let Some(scope) = &context.scope {
        list.push("--".into());
        list.push(scope.clone());
    }
    let result = run(context, &list, READ_TIMEOUT)?;
    if !result.status.success() {
        let error = String::from_utf8_lossy(&result.stderr).trim().to_string();
        if error.contains("not a git repository") {
            return Ok(GitState::default());
        }
        return Err(error);
    }
    let symbolic = read_text(context, &["symbolic-ref", "--short", "HEAD"]);
    let branch = if symbolic.is_empty() {
        "Detached HEAD".into()
    } else {
        symbolic
    };
    let head = read_text(context, &["rev-parse", "--verify", "-q", "HEAD"]);
    let refs = read_text(
        context,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--count=200",
            "--format=%(HEAD)%00%(refname:short)%00%(upstream:short)%00%(upstream:track,nobracket)",
            "refs/heads",
        ],
    );
    let mut state = GitState {
        entries: parse_status(&result.stdout),
        branch,
        repository: true,
        head,
        ..GitState::default()
    };
    apply_branch_refs(&mut state, &refs);
    if !state.upstream.is_empty() {
        state.upstream_head = read_text(context, &["rev-parse", "--verify", "-q", "@{upstream}"]);
    }
    Ok(state)
}

/// Branch names, and the current branch's upstream and distance from it, from
/// `for-each-ref --format=%(HEAD)%00%(refname:short)%00%(upstream:short)%00%(upstream:track,nobracket)`.
pub(crate) fn apply_branch_refs(state: &mut GitState, output: &str) {
    for line in output.lines() {
        let fields: Vec<&str> = line.split('\0').collect();
        let [current, name, upstream, track] = fields[..] else {
            continue;
        };
        state.branches.push(name.to_string());
        if current.trim() != "*" {
            continue;
        }
        // "gone" means the upstream was deleted: there is nothing to sync with.
        if track != "gone" {
            state.upstream = upstream.to_string();
        }
        for part in track.split(", ") {
            let count = |prefix: &str| {
                part.strip_prefix(prefix)
                    .and_then(|n| n.trim().parse().ok())
            };
            if let Some(n) = count("ahead ") {
                state.ahead = n;
            }
            if let Some(n) = count("behind ") {
                state.behind = n;
            }
        }
    }
}

/// Parse `git status --porcelain=v1 -z` output.
pub fn parse_status(output: &[u8]) -> Vec<GitEntry> {
    let mut records = output.split(|b| *b == 0);
    let mut entries = vec![];
    while let Some(record) = records.next() {
        if record.len() < 4 {
            continue;
        }
        let status = String::from_utf8_lossy(&record[..2]).into_owned();
        let raw = &record[3..];
        let original = if status.contains(['R', 'C']) {
            records.next()
        } else {
            None
        };
        let conflict = status.contains('U') || status == "AA" || status == "DD";
        let untracked = status == "??";
        for staged in [true, false] {
            if conflict || untracked {
                if staged {
                    continue;
                }
            } else if record[usize::from(!staged)] == b' ' {
                continue;
            }
            entries.push(GitEntry {
                status: status.clone(),
                path: String::from_utf8_lossy(raw).into_owned(),
                original_path: original.map(|p| String::from_utf8_lossy(p).into_owned()),
                staged,
                untracked,
                group: if conflict {
                    "Conflicts"
                } else if untracked {
                    "Untracked"
                } else if staged {
                    "Staged"
                } else {
                    "Unstaged"
                }
                .into(),
                raw_path: os(raw),
                raw_original: original.map(os),
            });
        }
    }
    entries.sort_by_key(|e| {
        (
            match e.group.as_str() {
                "Conflicts" => 0,
                "Staged" => 1,
                "Unstaged" => 2,
                _ => 3,
            },
            e.path.clone(),
        )
    });
    entries
}

/// The raw path for a displayed path, falling back to the text itself.
pub(crate) fn raw_path(entries: &[GitEntry], display: &str) -> OsString {
    entries
        .iter()
        .find(|e| e.path == display)
        .map(|e| e.raw_path.clone())
        .unwrap_or_else(|| display.into())
}

use crate::{services::Job, App};
use anyhow::{bail, Context as _, Result};

fn os_args(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

impl App {
    /// Git pane commands: staging, diffs and commits.
    pub(crate) fn git_command(&mut self, cmd: crate::Command) -> Result<()> {
        let Some(context) = self.git_ready(cmd.clone()) else {
            return Ok(());
        };
        match cmd {
            crate::Command::GitStage { path } => {
                let mut args: Vec<OsString> = vec!["add".into(), "--all".into(), "--".into()];
                match self.git.entries.iter().find(|e| e.path == path) {
                    Some(entry) => args.extend(entry.staging_paths()),
                    None => args.push(path.into()),
                }
                self.git_action(args)?;
            }
            crate::Command::GitUnstage { path } => {
                let mut args: Vec<OsString> = vec!["reset".into(), "HEAD".into(), "--".into()];
                args.push(raw_path(&self.git.entries, &path));
                if let Some(old) = self
                    .git
                    .entries
                    .iter()
                    .find(|e| e.path == path)
                    .and_then(|e| e.raw_original.clone())
                {
                    args.push(old);
                }
                self.git_action(args)?;
            }
            // Within the workspace, when the repository is larger.
            crate::Command::GitStageAll => {
                let mut args = os_args(&["add", "--all", "--"]);
                args.push(context.scope.clone().unwrap_or_else(|| ".".into()));
                self.git_action(args)?;
            }
            crate::Command::GitUnstageAll => {
                let mut args = os_args(&["reset", "HEAD", "--"]);
                args.push(context.scope.clone().unwrap_or_else(|| ".".into()));
                self.git_action(args)?;
            }
            crate::Command::GitStageGroup { group } => {
                let paths: std::collections::BTreeSet<OsString> = self
                    .git
                    .entries
                    .iter()
                    .filter(|entry| entry.group == group && !entry.staged)
                    .flat_map(GitEntry::staging_paths)
                    .collect();
                if paths.is_empty() {
                    bail!("No unstaged changes in {group}");
                }
                let mut args = os_args(&["add", "--all", "--"]);
                args.extend(paths);
                self.git_action(args)?;
            }
            crate::Command::GitDiff { path, staged } => {
                let untracked = self
                    .git
                    .entries
                    .iter()
                    .any(|e| e.path == path && e.untracked);
                self.services.tx.send(Job::Git(GitJob::Diff {
                    context,
                    path: raw_path(&self.git.entries, &path),
                    display: path,
                    staged,
                    untracked,
                }))?;
                self.git.jobs += 1;
                self.git.error.clear();
                self.status = "Loading Git diff…".into();
            }
            crate::Command::GitCommit { message } => {
                if message.trim().is_empty() {
                    bail!("Commit message is empty");
                }
                self.git_action(vec!["commit".into(), "-m".into(), message.into()])?;
            }
            _ => unreachable!("not a Git command"),
        }
        Ok(())
    }

    pub(crate) fn git_status(&mut self, state: Result<GitState, String>) {
        match state {
            Ok(state) => {
                self.git_status_finished();
                self.git.revision += 1;
                self.git.branch = state.branch;
                self.git.repository = state.repository;
                // A selected commit stays selected as the change list above it
                // grows or shrinks; a selected change is followed by its path.
                let commit = self.selected_commit().map(|c| c.hash.clone());
                let selection = self
                    .git
                    .entries
                    .get(self.git.selected)
                    .map(|e| (e.path.clone(), e.staged));
                self.git.entries = state.entries;
                if let Some(hash) = commit {
                    let index = self.git.history.iter().position(|c| c.hash == hash);
                    self.git.selected = self.git.entries.len() + index.unwrap_or(0);
                } else {
                    if let Some((path, staged)) = selection {
                        if let Some(index) = self
                            .git
                            .entries
                            .iter()
                            .position(|e| e.path == path && e.staged == staged)
                            .or_else(|| self.git.entries.iter().position(|e| e.path == path))
                        {
                            self.git.selected = index;
                        }
                    }
                    self.git.selected = self
                        .git
                        .selected
                        .min(self.git.entries.len().saturating_sub(1));
                }
                self.git.head = state.head;
                self.git.upstream = state.upstream;
                self.git.upstream_head = state.upstream_head;
                self.git.ahead = state.ahead;
                self.git.behind = state.behind;
                self.git.branches = state.branches;
                let key = (self.git.head.clone(), self.git.upstream_head.clone());
                if key != self.git.history_key || self.git.history_stale {
                    self.refresh_history();
                }
            }
            Err(e) => {
                self.git_status_finished();
                self.git.error = e.clone();
                self.status = format!("Git: {e}");
            }
        }
    }

    pub(crate) fn git_operation_done(&mut self, result: Result<String, String>) {
        self.git.jobs = self.git.jobs.saturating_sub(1);
        // Status runs on its own worker, so read it again only
        // once the operation has finished.
        self.refresh_git();
        match result {
            Ok(message) => {
                self.status = if message.is_empty() {
                    "Git operation completed".into()
                } else {
                    message.lines().next().unwrap_or_default().into()
                }
            }
            Err(e) => {
                self.git.error = e.clone();
                self.status = format!("Git: {e}");
            }
        }
    }

    pub(crate) fn git_status_finished(&mut self) {
        self.git.jobs = self.git.jobs.saturating_sub(1);
        self.git.status_running = false;
        if std::mem::take(&mut self.git.status_again) {
            self.refresh_git();
        }
    }
    /// The repository for the workspace and whether Git may run in it:
    /// (context, trusted). `None` outside a repository.
    pub(crate) fn git_context(&self) -> Option<(Context, bool)> {
        let top = find_repository(&self.root)?;
        let root = self
            .root
            .canonicalize()
            .unwrap_or_else(|_| self.root.clone());
        let scope = root
            .strip_prefix(&top)
            .ok()
            .filter(|rest| !rest.as_os_str().is_empty())
            .map(|rest| rest.as_os_str().to_os_string());
        let trusted = self.trusted_path(&top);
        Some((Context { root: top, scope }, trusted))
    }
    /// The context for a Git command, or `None` after asking to trust the
    /// repository (`retry` runs once it is trusted).
    pub(crate) fn git_ready(&mut self, retry: crate::Command) -> Option<Context> {
        match self.git_context() {
            None => {
                self.status = "Not a Git repository".into();
                None
            }
            Some((context, false)) => {
                let reason = if context.scope.is_some() {
                    "Git runs programs named by this repository's configuration (it contains the workspace)"
                } else {
                    "Git runs programs named by this repository's configuration"
                };
                let folder = context.root.clone();
                self.ask_trust_for(&folder, reason, retry);
                None
            }
            Some((context, true)) => Some(context),
        }
    }
    pub(crate) fn refresh_git(&mut self) {
        let (context, trusted) = match self.git_context() {
            Some(found) => found,
            None => {
                if self.git.repository || self.git.restricted || !self.git.entries.is_empty() {
                    self.git = Panel {
                        jobs: self.git.jobs,
                        revision: self.git.revision + 1,
                        // Revisions only grow, so frontends never keep a stale graph.
                        history_revision: self.git.history_revision + 1,
                        ..Panel::default()
                    };
                }
                return;
            }
        };
        let top = context.root.to_string_lossy();
        if self.git.top != top {
            self.git.top = top.into_owned();
        }
        if !trusted {
            if !self.git.restricted {
                self.git.entries.clear();
                self.git.branch.clear();
                self.git.selected = 0;
                self.git.repository = true;
                self.git.restricted = true;
                self.clear_repository_details();
                self.git.error = format!(
                    "Git is off until you trust {}: repository configuration can run programs.",
                    context.root.display()
                );
                self.git.revision += 1;
            }
            return;
        }
        if self.git.restricted {
            self.git.restricted = false;
            self.git.error.clear();
            self.git.revision += 1;
        }
        // Status reads are coalesced: one runs at a time, and requests made
        // meanwhile become a single follow-up read.
        if self.git.status_running {
            self.git.status_again = true;
            return;
        }
        if self
            .services
            .tx
            .send(Job::Git(GitJob::Status(context)))
            .is_ok()
        {
            self.git.status_running = true;
            self.git.jobs += 1;
        }
    }
    /// Ask to trust the folder Git needs (the repository, else the
    /// workspace).
    pub(crate) fn request_trust(&mut self) {
        match self.git_context() {
            Some((context, false)) => {
                let folder = context.root.clone();
                self.ask_trust_for(
                    &folder,
                    "Trust it to use Git and project tools",
                    crate::Command::Refresh,
                );
            }
            _ if !self.trusted => {
                self.ask_trust("Trust it to use project tools", crate::Command::Refresh)
            }
            _ => self.status = "This folder is already trusted".into(),
        }
    }
    /// The Git pane's branch, remote and per-file actions.
    pub(crate) fn git_pane_action(&mut self, name: &str, argument: &str) -> Result<()> {
        match name {
            "git-fetch" | "git-pull" | "git-push" => {
                let (remote, doing) = match name {
                    "git-fetch" => (Remote::Fetch, "Fetching"),
                    "git-pull" => (Remote::Pull, "Pulling"),
                    _ => (Remote::Push, "Pushing"),
                };
                let retry = crate::Command::Action {
                    name: name.into(),
                    argument: String::new(),
                };
                let Some(context) = self.git_ready(retry) else {
                    return Ok(());
                };
                self.services
                    .tx
                    .send(Job::Git(GitJob::Remote(context, remote)))?;
                self.git.error.clear();
                self.git.jobs += 1;
                self.status = format!("{doing}…");
            }
            "git-switch" | "git-branch" => {
                let branch = argument.trim();
                if branch.is_empty()
                    || branch.starts_with('-')
                    || branch.chars().any(|c| c.is_whitespace() || c.is_control())
                {
                    bail!("Not a branch name: {branch}");
                }
                let mut list = os_args(&["switch"]);
                if name == "git-branch" {
                    list.push("--create".into());
                }
                list.push(branch.into());
                self.git_action(list)?;
            }
            "git-show" => self.show_commit(argument)?,
            "git-history-more" => self.load_more_history()?,
            "git-open" => {
                let path = self.selected_change_path()?;
                if !path.exists() {
                    bail!("{} was deleted", path.display());
                }
                self.execute(crate::Command::Open { path })?;
            }
            "discard-changes" => {
                let entry = self.discardable()?.clone();
                let path = self.selected_change_path()?;
                self.ensure_change_unedited(&path)?;
                self.pending_file = Some(path);
                self.prompt = Some(crate::search::Prompt {
                    kind: "discard-changes".into(),
                    input: entry.path.clone(),
                    replacement: if entry.untracked {
                        "untracked".into()
                    } else {
                        String::new()
                    },
                    field: 0,
                    case_sensitive: false,
                    whole_word: false,
                });
            }
            "reveal-in-files" => {
                let target = if !argument.trim().is_empty() {
                    PathBuf::from(argument.trim())
                } else if self.focused_kind() == "git" {
                    self.selected_change_path()?
                } else {
                    self.active_editor()
                        .and_then(|id| self.views.get(&id))
                        .and_then(|v| self.documents[&v.document].path.clone())
                        .ok_or_else(|| anyhow::anyhow!("Select a file to reveal"))?
                };
                self.reveal_in_files(&target)?;
            }
            _ => bail!("Unknown Git action: {name}"),
        }
        Ok(())
    }
    /// The selected change's file, in the working tree.
    pub(crate) fn selected_change_path(&self) -> Result<PathBuf> {
        let entry = self
            .git
            .entries
            .get(self.git.selected)
            .ok_or_else(|| anyhow::anyhow!("Select a changed file first"))?;
        if self.git.top.is_empty() {
            bail!("Git is not available here");
        }
        Ok(Path::new(&self.git.top).join(&entry.raw_path))
    }
    /// The selected change, when it is a working-tree change Git can undo.
    fn discardable(&self) -> Result<&GitEntry> {
        let entry = self
            .git
            .entries
            .get(self.git.selected)
            .ok_or_else(|| anyhow::anyhow!("Select a changed file first"))?;
        if entry.staged {
            bail!("Unstage the change first; only working changes are discarded");
        }
        if entry.group == "Conflicts" {
            bail!("Resolve conflicts in the file; they cannot be discarded");
        }
        Ok(entry)
    }
    fn ensure_change_unedited(&self, path: &Path) -> Result<()> {
        if self
            .documents
            .values()
            .any(|doc| doc.path.as_deref() == Some(path) && doc.dirty())
        {
            bail!("Save or close the modified document before discarding its changes");
        }
        Ok(())
    }
    /// Discard the change the "discard-changes" prompt named: restore a
    /// tracked file from the index, or move an untracked one to the Trash.
    pub(crate) fn confirm_discard(&mut self) -> Result<()> {
        let path = self.pending_file.take().context("No change selected")?;
        let entry = self
            .git
            .entries
            .iter()
            .find(|e| !e.staged && Path::new(&self.git.top).join(&e.raw_path) == path)
            .cloned()
            .context("The change is no longer listed")?;
        self.ensure_change_unedited(&path)?;
        if entry.untracked {
            self.services
                .io
                .send(crate::services::IoJob::FileOperation(
                    crate::files::FileOperation::Trash(path),
                ))?;
            self.status = "Moving the untracked file to Trash…".into();
            return Ok(());
        }
        let mut list = os_args(&["restore", "--worktree", "--"]);
        list.extend(entry.staging_paths());
        self.git_action(list)
    }
    pub(crate) fn git_action(&mut self, args: Vec<OsString>) -> Result<()> {
        let Some((context, true)) = self.git_context() else {
            bail!("Git is not available here");
        };
        self.services
            .tx
            .send(Job::Git(GitJob::Run(context, args)))?;
        self.git.error.clear();
        self.git.jobs += 1;
        self.status = "Running Git…".into();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hung_git_is_killed_after_its_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let context = Context {
            root: dir.path().into(),
            scope: None,
        };
        // An alias that sleeps stands in for a hung helper.
        let started = std::time::Instant::now();
        let result = run(
            &context,
            &args(&["-c", "alias.hang=!sleep 30", "hang"]),
            Duration::from_millis(300),
        );
        assert!(result.unwrap_err().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn git_ignores_a_parent_gits_repository_environment() {
        // Set when Slate is Git's commit editor; `-C root` must decide.
        let command = command(Path::new("/project"));
        let envs: Vec<_> = command.get_envs().collect();
        for name in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_PREFIX"] {
            assert!(
                envs.contains(&(OsStr::new(name), None)),
                "{name} is inherited"
            );
        }
        assert!(envs.contains(&(OsStr::new("GIT_OPTIONAL_LOCKS"), Some(OsStr::new("0")))));
    }

    #[test]
    fn branch_refs_give_upstream_distance_and_names() {
        let mut state = GitState::default();
        apply_branch_refs(
            &mut state,
            " \0topic\0origin/topic\0behind 1\n*\0main\0origin/main\0ahead 2, behind 3\n \0old\0origin/old\0gone",
        );
        assert_eq!(state.branches, ["topic", "main", "old"]);
        assert_eq!(state.upstream, "origin/main");
        assert_eq!((state.ahead, state.behind), (2, 3));
        // A deleted upstream leaves nothing to sync with.
        let mut gone = GitState::default();
        apply_branch_refs(&mut gone, "*\0old\0origin/old\0gone");
        assert!(gone.upstream.is_empty() && gone.ahead == 0);
        // Detached HEAD: no branch is current.
        let mut detached = GitState::default();
        apply_branch_refs(&mut detached, " \0main\0\0");
        assert!(detached.upstream.is_empty() && detached.branches == ["main"]);
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_paths_keep_their_bytes() {
        let entries = parse_status(b"?? caf\xe9.txt\0R  new.txt\0old\xff.txt\0");
        let untracked = entries.iter().find(|e| e.untracked).unwrap();
        assert_eq!(untracked.path, "caf\u{fffd}.txt");
        use std::os::unix::ffi::OsStrExt;
        assert_eq!(untracked.raw_path.as_bytes(), b"caf\xe9.txt");
        let renamed = entries.iter().find(|e| e.staged).unwrap();
        assert_eq!(
            renamed.raw_original.as_ref().unwrap().as_bytes(),
            b"old\xff.txt"
        );
    }
}
