//! Git integration. Commands run with a timeout so a hung credential helper
//! or a huge repository cannot wedge the Git pane, paths keep their raw
//! bytes so files with non-UTF-8 names can still be staged, and untrusted
//! workspaces never run repository-configured programs (fsmonitor, hooks).
use serde::Serialize;
use std::{
    ffi::{OsStr, OsString},
    io::Read,
    path::PathBuf,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
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
        }
    }
}

/// Where and how to run Git.
#[derive(Clone)]
pub struct Context {
    pub root: PathBuf,
    pub trusted: bool,
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
}

pub enum GitReply {
    Status(Result<GitState, String>),
    Operation(Result<String, String>),
    Output(String, String),
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

/// Run git, killing it (and anything it started) after `timeout`.
pub fn run(context: &Context, args: &[OsString], timeout: Duration) -> Result<Output, String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(&context.root);
    if !context.trusted {
        // Repository configuration can name programs to run. A folder the
        // user has not trusted must not execute them.
        command.args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "diff.external=",
        ]);
    }
    command
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        // Status must not take the index lock away from the user's own Git.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|e| format!("git: {e}"))?;
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            bytes
        })
    };
    let stdout = drain(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let stderr = drain(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => std::thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                #[cfg(unix)]
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "git {} timed out after {} s",
                    args.first()
                        .map(|a| a.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    timeout.as_secs()
                ));
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    Ok(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

fn args(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

/// The repository root containing `context.root`, or the root itself.
fn toplevel(context: &Context) -> Context {
    let root = run(
        context,
        &args(&["rev-parse", "--show-toplevel"]),
        READ_TIMEOUT,
    )
    .ok()
    .filter(|r| r.status.success())
    .map(|r| {
        let mut out = r.stdout;
        while out.last().is_some_and(|b| *b == b'\n' || *b == b'\r') {
            out.pop();
        }
        PathBuf::from(os(&out))
    })
    .unwrap_or_else(|| context.root.clone());
    Context {
        root,
        trusted: context.trusted,
    }
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
        } => diff(&context, path, &display, staged, untracked),
        GitJob::Run(context, args) => GitReply::Operation(operation(&context, args)),
    }
}

fn operation(context: &Context, mut list: Vec<OsString>) -> Result<String, String> {
    let context = toplevel(context);
    if list.first().is_some_and(|a| a == "reset")
        && !run(
            &context,
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
    match run(&context, &list, WRITE_TIMEOUT)? {
        result if result.status.success() => {
            Ok(String::from_utf8_lossy(&result.stdout).trim().to_string())
        }
        result => Err(String::from_utf8_lossy(&result.stderr).trim().to_string()),
    }
}

fn diff(
    context: &Context,
    path: OsString,
    display: &str,
    staged: bool,
    untracked: bool,
) -> GitReply {
    let context = toplevel(context);
    let mut list = args(&["diff", "--no-ext-diff", "--no-textconv"]);
    if untracked {
        list.extend(args(&["--no-index", "--", "/dev/null"]));
        list.push(path);
    } else {
        if staged {
            list.push("--cached".into());
        }
        list.push("--".into());
        list.push(path);
    }
    match run(&context, &list, READ_TIMEOUT) {
        Ok(result) if result.status.success() || (untracked && result.status.code() == Some(1)) => {
            let mut text = String::from_utf8_lossy(&result.stdout).into_owned();
            if text.is_empty() {
                text =
                    "No changes in this comparison. Refresh Git if the file changed externally.\n"
                        .into();
            }
            if text.len() > 2 * 1024 * 1024 {
                let mut end = 2 * 1024 * 1024;
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                text.truncate(end);
                text.push_str("\n[Diff preview truncated at 2 MiB]\n");
            }
            let kind = if untracked {
                "Untracked"
            } else if staged {
                "Staged diff"
            } else {
                "Working diff"
            };
            GitReply::Output(format!("{kind} · {display}"), text)
        }
        Ok(result) => {
            GitReply::Operation(Err(String::from_utf8_lossy(&result.stderr).trim().into()))
        }
        Err(e) => GitReply::Operation(Err(e)),
    }
}

fn status(context: &Context) -> Result<GitState, String> {
    let context = toplevel(context);
    let result = run(
        &context,
        &args(&["status", "--porcelain=v1", "--untracked-files=all", "-z"]),
        READ_TIMEOUT,
    )?;
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
        &context,
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
        match cmd {
            crate::Command::GitStage { .. }
            | crate::Command::GitUnstage { .. }
            | crate::Command::GitStageAll
            | crate::Command::GitUnstageAll
            | crate::Command::GitStageGroup { .. }
            | crate::Command::GitCommit { .. }
                if !self.trusted =>
            {
                self.ask_trust(
                    "Git staging and commits run this repository's hooks and filters",
                    cmd,
                );
            }
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
            crate::Command::GitStageAll => {
                self.git_action(os_args(&["add", "--all", "--", "."]))?;
            }
            crate::Command::GitUnstageAll => {
                self.git_action(os_args(&["reset", "HEAD", "--", "."]))?;
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
                    context: self.git_context(),
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
    pub(crate) fn git_context(&self) -> Context {
        Context {
            root: self.root.clone(),
            trusted: self.trusted,
        }
    }
    pub(crate) fn refresh_git(&mut self) {
        // Status reads are coalesced: one runs at a time, and requests made
        // meanwhile become a single follow-up read.
        if self.git.status_running {
            self.git.status_again = true;
            return;
        }
        if self
            .services
            .tx
            .send(Job::Git(GitJob::Status(self.git_context())))
            .is_ok()
        {
            self.git.status_running = true;
            self.git.jobs += 1;
        }
    }
    pub(crate) fn git_action(&mut self, args: Vec<OsString>) -> Result<()> {
        self.services
            .tx
            .send(Job::Git(GitJob::Run(self.git_context(), args)))?;
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
            trusted: true,
        };
        // An alias that sleeps stands in for a hung helper.
        let started = Instant::now();
        let result = run(
            &context,
            &args(&["-c", "alias.hang=!sleep 30", "hang"]),
            Duration::from_millis(300),
        );
        assert!(result.unwrap_err().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(5));
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
