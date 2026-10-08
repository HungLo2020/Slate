use crate::{
    document::Document,
    highlight,
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
pub use crate::git::{GitEntry, GitState};
pub enum Job {
    Browse(PathBuf),
    Git(crate::git::GitJob),
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
    /// Compare open files with their baselines (the external-change watcher).
    #[allow(clippy::type_complexity)]
    CheckDisk(
        Vec<(
            u64,
            PathBuf,
            Option<crate::fsio::Baseline>,
            Option<crate::document::FileStamp>,
            String,
        )>,
    ),
    Checkpoint(PathBuf, Workspace),
    Flush(mpsc::SyncSender<()>),
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
    Highlighted(highlight::Response),
    Files(PathBuf, Result<Vec<Entry>, String>),
    Git(Result<GitState, String>),
    GitOperation(Result<String, String>),
    Output(String, String),
    Error(String),
    Spelling(Result<Vec<String>, String>),
    Disk(Vec<(u64, crate::document::DiskChange)>),
    /// The workspace file index for a root.
    Index(PathBuf, Vec<String>),
    SearchHits(u64, Vec<crate::project::Hit>),
    SearchDone(u64, Result<usize, String>),
    /// Grammar-based symbols of a document.
    Outline(u64, Vec<crate::outline::Symbol>),
    /// Files rewritten by replace-in-files.
    Replaced(crate::picker::ReplaceReport),
    /// A formatter's output: (document, content version, text).
    Formatted(u64, u64, Result<String, String>),
    /// An external tool finished: (name, output mode, target, result).
    #[allow(clippy::type_complexity)]
    ToolDone(
        String,
        crate::tools::Output,
        Option<(u64, u64, (usize, usize))>,
        Result<String, String>,
    ),
    /// Preview lines for a location list: (generation, (entry, line)).
    Previews(u64, Vec<(usize, String)>),
    /// A message from an IDE service (language server, debugger, task).
    Ide(crate::ide::Event),
}
#[derive(Clone)]
pub struct ReplySender {
    sender: Sender<Reply>,
    events: crate::events::Events,
}
impl ReplySender {
    pub fn send(&self, reply: Reply) -> Result<(), ()> {
        self.sender.send(reply).map_err(|_| ())?;
        self.events.notify();
        Ok(())
    }
}
pub struct JobSender {
    /// Status and diffs; a slow status never delays staging or commits.
    git_read: Sender<Job>,
    git_write: Sender<Job>,
    files: Sender<Job>,
}
impl JobSender {
    pub fn send(&self, job: Job) -> Result<(), mpsc::SendError<Job>> {
        use crate::git::GitJob;
        match job {
            Job::Git(GitJob::Run(..)) => self.git_write.send(job),
            Job::Git(_) => self.git_read.send(job),
            _ => self.files.send(job),
        }
    }
}
pub struct Services {
    output: ReplySender,
    pub tx: JobSender,
    pub rx: Receiver<Reply>,
    pub io: Sender<IoJob>,
    pub highlight: mpsc::SyncSender<highlight::Request>,
    /// The newest requested highlight generation per document.
    pub highlight_latest: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<u64, u64>>>,
}
impl Services {
    pub fn new(events: crate::events::Events) -> Self {
        let (git_read, read_jobs) = mpsc::channel();
        let (git_write, write_jobs) = mpsc::channel();
        let (files, file_jobs) = mpsc::channel();
        let tx = JobSender {
            git_read,
            git_write,
            files,
        };
        let (sender, rx) = mpsc::channel();
        let output = ReplySender { sender, events };
        let output_root = output.clone();
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
                            crate::fsio::write_elevated(
                                &path,
                                &bytes,
                                doc.disk.as_ref(),
                                &program,
                                false,
                            )?;
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
                    IoJob::CheckDisk(entries) => {
                        let changes: Vec<_> = entries
                            .into_iter()
                            .filter_map(|(id, path, baseline, stamp, encoding)| {
                                crate::document::examine(&path, baseline.as_ref(), stamp, &encoding)
                                    .map(|change| (id, change))
                            })
                            .collect();
                        Reply::Disk(changes)
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
        let (highlight, requests) = mpsc::sync_channel::<highlight::Request>(64);
        let highlight_latest = std::sync::Arc::new(std::sync::Mutex::new(Default::default()));
        let highlight_output = output.clone();
        let latest = highlight_latest.clone();
        thread::spawn(move || {
            highlight::worker(requests, latest, |response| {
                highlight_output.send(Reply::Highlighted(response)).is_ok()
            });
        });
        for jobs in [read_jobs, write_jobs, file_jobs] {
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
                        Job::Spell(text) => Reply::Spelling(spell(&text)),
                        Job::Git(job) => match crate::git::handle(job) {
                            crate::git::GitReply::Status(state) => Reply::Git(state),
                            crate::git::GitReply::Operation(result) => Reply::GitOperation(result),
                            crate::git::GitReply::Output(title, text) => Reply::Output(title, text),
                        },
                    };
                    if output.send(reply).is_err() {
                        break;
                    }
                }
            });
        }
        Self {
            output: output_root,
            tx,
            rx,
            io,
            highlight,
            highlight_latest,
        }
    }
}

impl Services {
    /// Run `work` on its own thread and deliver its reply.
    pub fn background(&self, work: impl FnOnce() -> Reply + Send + 'static) {
        let output = self.output.clone();
        thread::spawn(move || {
            let _ = output.send(work());
        });
    }
    /// A handle for threads that deliver several replies.
    pub fn reply_sender(&self) -> ReplySender {
        self.output.clone()
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
