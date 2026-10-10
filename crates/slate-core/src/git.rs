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
pub struct GitState {
    pub entries: Vec<GitEntry>,
    pub branch: String,
    pub repository: bool,
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
}

pub enum GitReply {
    Status(Result<GitState, String>),
    Operation(Result<String, String>),
    Preview(Result<crate::comparison::Preview, String>),
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
    let label = format!(
        "git {}",
        args.first()
            .map(|a| a.to_string_lossy().into_owned())
            .unwrap_or_default()
    );
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
            return Ok(GitState {
                entries: vec![],
                branch: String::new(),
                repository: false,
            });
        }
        return Err(error);
    }
    let branch = run(
        context,
        &args(&["symbolic-ref", "--short", "HEAD"]),
        READ_TIMEOUT,
    )
    .ok()
    .filter(|r| r.status.success())
    .map(|r| String::from_utf8_lossy(&r.stdout).trim().to_string())
    .unwrap_or_else(|| "Detached HEAD".into());
    Ok(GitState {
        entries: parse_status(&result.stdout),
        branch,
        repository: true,
    })
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
use anyhow::{bail, Result};

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
                let selection = self
                    .git
                    .entries
                    .get(self.git.selected)
                    .map(|e| (e.path.clone(), e.staged));
                self.git.entries = state.entries;
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
                        ..Panel::default()
                    };
                }
                return;
            }
        };
        if !trusted {
            if !self.git.restricted {
                self.git.entries.clear();
                self.git.branch.clear();
                self.git.selected = 0;
                self.git.repository = true;
                self.git.restricted = true;
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
