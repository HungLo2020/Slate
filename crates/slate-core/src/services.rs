use crate::{
    document::Document,
    highlight::{self, Token},
    workspace::{self, Workspace},
};
use serde::Serialize;
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::mpsc::{self, Receiver, Sender},
    thread,
};
#[derive(Clone, Serialize, PartialEq)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub directory: bool,
}
#[derive(Clone, Serialize, PartialEq)]
pub struct GitEntry {
    pub status: String,
    pub path: String,
    pub original_path: Option<String>,
    pub staged: bool,
    pub untracked: bool,
    pub group: String,
}
impl GitEntry {
    pub(crate) fn staging_paths(&self) -> impl Iterator<Item = String> {
        // An index rename already removed the source. Passing that missing
        // source to `git add` rejects a later edit to the destination. Include
        // both paths only when the rename still belongs to the working tree.
        let source = self.original_path.clone().filter(|_| {
            self.status.as_bytes().get(1) == Some(&b'R') && !self.status.starts_with('R')
        });
        std::iter::once(self.path.clone()).chain(source)
    }
}
pub struct GitState {
    pub entries: Vec<GitEntry>,
    pub branch: String,
    pub repository: bool,
}
pub enum Job {
    Browse(PathBuf),
    Git(PathBuf, Vec<String>),
    GitStatus(PathBuf),
    GitDiff(PathBuf, String, bool, bool),
    /// List misspelled words with aspell, hunspell or enchant.
    Spell(String),
}
pub enum IoJob {
    Open(u64, PathBuf),
    /// Reread a document's file, optionally decoding with a chosen encoding.
    Reload(u64, PathBuf, Option<String>),
    Save(
        u64,
        Document,
        Option<PathBuf>,
        bool,
        crate::fsio::WriteOptions,
    ),
    /// Write through `pkexec`/`sudo` without a terminal (desktop sessions).
    SaveElevated(u64, Document, String),
    Checkpoint(PathBuf, Workspace),
    Flush(mpsc::SyncSender<()>),
}
pub struct HighlightJob {
    pub id: u64,
    pub generation: u64,
    pub text: String,
    pub path: Option<PathBuf>,
    pub light: bool,
}
pub struct SaveFailure {
    pub kind: crate::fsio::SaveErrorKind,
    pub message: String,
}
pub enum Reply {
    Opened(u64, Result<Document, String>),
    Reloaded(u64, Result<Document, String>),
    OpenedDirectory(u64, PathBuf),
    Saved(u64, Result<Document, SaveFailure>),
    Highlighted(u64, u64, bool, Vec<Token>),
    Files(PathBuf, Result<Vec<Entry>, String>),
    Git(Result<GitState, String>),
    GitOperation(Result<String, String>),
    Output(String, String),
    Error(String),
    Spelling(Result<Vec<String>, String>),
}
#[derive(Clone)]
struct ReplySender {
    sender: Sender<Reply>,
    events: crate::events::Events,
}
impl ReplySender {
    fn send(&self, reply: Reply) -> Result<(), ()> {
        self.sender.send(reply).map_err(|_| ())?;
        self.events.notify();
        Ok(())
    }
}
pub struct JobSender {
    git: Sender<Job>,
    files: Sender<Job>,
}
impl JobSender {
    pub fn send(&self, job: Job) -> Result<(), mpsc::SendError<Job>> {
        if matches!(job, Job::Browse(_) | Job::Spell(_)) {
            self.files.send(job)
        } else {
            self.git.send(job)
        }
    }
}
pub struct Services {
    pub tx: JobSender,
    pub rx: Receiver<Reply>,
    pub io: Sender<IoJob>,
    pub highlight: mpsc::SyncSender<HighlightJob>,
}
impl Services {
    pub fn new(events: crate::events::Events) -> Self {
        let (git, jobs) = mpsc::channel();
        let (files, file_jobs) = mpsc::channel();
        let tx = JobSender { git, files };
        let (sender, rx) = mpsc::channel();
        let output = ReplySender { sender, events };
        let (io, io_jobs) = mpsc::channel();
        let io_output = output.clone();
        thread::spawn(move || {
            while let Ok(job) = io_jobs.recv() {
                let reply = match job {
                    IoJob::Open(id, path) => {
                        if path.is_dir() {
                            Reply::OpenedDirectory(id, path)
                        } else {
                            Reply::Opened(id, Document::open(&path).map_err(|e| format!("{e:#}")))
                        }
                    }
                    IoJob::Reload(id, path, encoding) => Reply::Reloaded(
                        id,
                        Document::open_with_encoding(&path, encoding.as_deref())
                            .map_err(|e| format!("{e:#}")),
                    ),
                    IoJob::Save(id, mut doc, destination, overwrite, options) => Reply::Saved(
                        id,
                        doc.save_with_options(destination.as_deref(), overwrite, options)
                            .map(|()| doc)
                            .map_err(|e| SaveFailure {
                                kind: crate::fsio::error_kind(&e),
                                message: format!("{e:#}"),
                            }),
                    ),
                    IoJob::SaveElevated(id, mut doc, program) => Reply::Saved(
                        id,
                        (|| -> anyhow::Result<Document> {
                            let path = doc.path.clone().ok_or_else(|| {
                                anyhow::anyhow!("Use Save As for an untitled document")
                            })?;
                            let bytes = doc.encoded()?;
                            crate::fsio::write_elevated(&path, &bytes, &program, false)?;
                            doc.mark_saved_bytes(&bytes);
                            Ok(doc)
                        })()
                        .map_err(|e| SaveFailure {
                            kind: crate::fsio::SaveErrorKind::Other,
                            message: format!("{e:#}"),
                        }),
                    ),
                    IoJob::Checkpoint(path, w) => {
                        if let Err(e) = workspace::write_checkpoint(&path, &w) {
                            let _ = io_output
                                .send(Reply::Error(format!("Recovery checkpoint failed: {e:#}")));
                        }
                        continue;
                    }
                    IoJob::Flush(done) => {
                        let _ = done.send(());
                        continue;
                    }
                };
                if io_output.send(reply).is_err() {
                    break;
                }
            }
        });
        let (highlight, requests) = mpsc::sync_channel::<HighlightJob>(1);
        let highlight_output = output.clone();
        thread::spawn(move || {
            while let Ok(job) = requests.recv() {
                let tokens = highlight::highlight(&job.text, job.path.as_deref(), job.light);
                if highlight_output
                    .send(Reply::Highlighted(
                        job.id,
                        job.generation,
                        job.light,
                        tokens,
                    ))
                    .is_err()
                {
                    break;
                }
            }
        });
        for jobs in [jobs, file_jobs] {
            let output = output.clone();
            thread::spawn(move || {
                while let Ok(job) = jobs.recv() {
                    let reply = match job {
                        Job::Browse(path) => {
                            let entries = (|| -> std::io::Result<Vec<Entry>> {
                                let mut list = vec![];
                                if let Some(parent) = path.parent() {
                                    list.push(Entry {
                                        name: "..".into(),
                                        path: parent.to_string_lossy().into_owned(),
                                        directory: true,
                                    });
                                }
                                let mut children = vec![];
                                for entry in fs::read_dir(&path)?.take(100_000) {
                                    let e = entry?;
                                    children.push(Entry {
                                        name: e.file_name().to_string_lossy().into_owned(),
                                        path: e.path().to_string_lossy().into_owned(),
                                        directory: e.path().is_dir(),
                                    });
                                }
                                children.sort_by(|a, b| {
                                    b.directory.cmp(&a.directory).then_with(|| {
                                        a.name.to_lowercase().cmp(&b.name.to_lowercase())
                                    })
                                });
                                list.extend(children);
                                Ok(list)
                            })();
                            Reply::Files(path, entries.map_err(|e| e.to_string()))
                        }
                        Job::GitStatus(root) => Reply::Git(git_status(&root)),
                        Job::Spell(text) => Reply::Spelling(spell(&text)),
                        Job::GitDiff(root, path, staged, untracked) => {
                            let root = git_root(&root);
                            let mut args = vec![
                                "diff".to_string(),
                                "--no-ext-diff".into(),
                                "--no-textconv".into(),
                            ];
                            if untracked {
                                args.extend([
                                    "--no-index".into(),
                                    "--".into(),
                                    "/dev/null".into(),
                                    path.clone(),
                                ]);
                            } else {
                                if staged {
                                    args.push("--cached".into());
                                }
                                args.extend(["--".into(), path.clone()]);
                            }
                            match git_output(&root, &args) {
                                Ok(result)
                                    if result.status.success()
                                        || (untracked && result.status.code() == Some(1)) =>
                                {
                                    let mut text =
                                        String::from_utf8_lossy(&result.stdout).into_owned();
                                    if text.is_empty() {
                                        text = "No changes in this comparison. Refresh Git if the file changed externally.\n".into();
                                    }
                                    if text.len() > 2 * 1024 * 1024 {
                                        let mut end = 2 * 1024 * 1024;
                                        while !text.is_char_boundary(end) {
                                            end -= 1;
                                        }
                                        text.truncate(end);
                                        text.push_str("\n[Diff preview truncated at 2 MiB]\n");
                                    }
                                    Reply::Output(
                                        format!(
                                            "{} · {}",
                                            if untracked {
                                                "Untracked"
                                            } else if staged {
                                                "Staged diff"
                                            } else {
                                                "Working diff"
                                            },
                                            path
                                        ),
                                        text,
                                    )
                                }
                                Ok(result) => Reply::GitOperation(Err(String::from_utf8_lossy(
                                    &result.stderr,
                                )
                                .trim()
                                .into())),
                                Err(e) => Reply::GitOperation(Err(e)),
                            }
                        }
                        Job::Git(root, mut args) => {
                            let root = git_root(&root);
                            if args.first().map(String::as_str) == Some("reset")
                                && !git_output(
                                    &root,
                                    &["rev-parse".into(), "--verify".into(), "HEAD".into()],
                                )
                                .is_ok_and(|r| r.status.success())
                            {
                                let paths = args
                                    .iter()
                                    .position(|a| a == "--")
                                    .map(|i| args[i + 1..].to_vec())
                                    .unwrap_or_default();
                                // An unborn index has no HEAD to reset to. Remove
                                // its entries only, including nested paths and
                                // entries whose working copy has changed again.
                                args = vec![
                                    "rm".into(),
                                    "--cached".into(),
                                    "-r".into(),
                                    "--force".into(),
                                    "--".into(),
                                ];
                                args.extend(paths);
                            }
                            Reply::GitOperation(match git_output(&root, &args) {
                                Ok(result) if result.status.success() => {
                                    Ok(String::from_utf8_lossy(&result.stdout).trim().to_string())
                                }
                                Ok(result) => {
                                    Err(String::from_utf8_lossy(&result.stderr).trim().to_string())
                                }
                                Err(e) => Err(e),
                            })
                        }
                    };
                    if output.send(reply).is_err() {
                        break;
                    }
                }
            });
        }
        Self {
            tx,
            rx,
            io,
            highlight,
        }
    }
}

/// Misspelled words, in document order without duplicates.
fn spell(text: &str) -> Result<Vec<String>, String> {
    use std::io::Write;
    let checkers: [(&str, &[&str]); 3] = [
        ("aspell", &["list"]),
        ("hunspell", &["-l"]),
        ("enchant-2", &["-l"]),
    ];
    let (program, args) = checkers
        .iter()
        .find(|(program, _)| crate::fsio::which(program).is_some())
        .ok_or("Install aspell, hunspell or enchant to check spelling")?;
    let mut child = Command::new(program)
        .args(*args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("No spell checker input")?;
    let input = text.to_string();
    let writer = thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes());
    });
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    let _ = writer.join();
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let mut seen = std::collections::BTreeSet::new();
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|w| !w.is_empty() && seen.insert(w.to_string()))
        .map(String::from)
        .collect())
}

fn git_output(root: &std::path::Path, args: &[String]) -> Result<std::process::Output, String> {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| e.to_string())
}
fn git_root(root: &std::path::Path) -> PathBuf {
    git_output(root, &["rev-parse".into(), "--show-toplevel".into()])
        .ok()
        .filter(|r| r.status.success())
        .map(|r| PathBuf::from(String::from_utf8_lossy(&r.stdout).trim()))
        .unwrap_or_else(|| root.to_path_buf())
}
fn git_status(root: &std::path::Path) -> Result<GitState, String> {
    let root = git_root(root);
    let result = git_output(
        &root,
        &[
            "status".into(),
            "--porcelain=v1".into(),
            "--untracked-files=all".into(),
            "-z".into(),
        ],
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
    let branch = git_output(
        &root,
        &["symbolic-ref".into(), "--short".into(), "HEAD".into()],
    )
    .ok()
    .filter(|r| r.status.success())
    .map(|r| String::from_utf8_lossy(&r.stdout).trim().to_string())
    .unwrap_or_else(|| "Detached HEAD".into());
    let mut records = result.stdout.split(|b| *b == 0);
    let mut entries = vec![];
    while let Some(record) = records.next() {
        if record.len() < 4 {
            continue;
        }
        let status = String::from_utf8_lossy(&record[..2]).into_owned();
        let path = String::from_utf8_lossy(&record[3..]).into_owned();
        let original_path = if status.contains(['R', 'C']) {
            records
                .next()
                .map(|p| String::from_utf8_lossy(p).into_owned())
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
                path: path.clone(),
                original_path: original_path.clone(),
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
    Ok(GitState {
        entries,
        branch,
        repository: true,
    })
}
