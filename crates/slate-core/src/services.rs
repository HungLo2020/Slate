use crate::{
    document::Document,
    highlight,
    workspace::{self, Workspace},
};
use serde::Serialize;
use std::{
    path::PathBuf,
    process::Command,
    sync::mpsc::{self, Receiver},
    thread,
};

/// A single cancellable service keeps the latest queued request, never a thread per key.
type LatestState<T> = std::sync::Arc<(
    std::sync::Mutex<(Option<T>, bool, bool)>,
    std::sync::Condvar,
)>;
pub(crate) struct LatestWorker<T> {
    state: LatestState<T>,
    thread: Option<thread::JoinHandle<()>>,
}
impl<T: Send + 'static> LatestWorker<T> {
    pub(crate) fn new(mut run: impl FnMut(T) + Send + 'static) -> Self {
        let state = std::sync::Arc::new((
            std::sync::Mutex::new((None, false, false)),
            std::sync::Condvar::new(),
        ));
        let inbox = state.clone();
        let thread = thread::spawn(move || loop {
            let (lock, changed) = &*inbox;
            let mut state = lock.lock().unwrap();
            while state.0.is_none() && !state.1 {
                state = changed.wait(state).unwrap();
            }
            if state.1 {
                let last = if state.2 { state.0.take() } else { None };
                drop(state);
                if let Some(last) = last {
                    run(last);
                }
                break;
            }
            let mut request = state.0.take().unwrap();
            drop(state);
            thread::sleep(std::time::Duration::from_millis(35));
            let mut state = lock.lock().unwrap();
            if state.1 {
                if state.2 {
                    request = state.0.take().unwrap_or(request);
                    drop(state);
                    run(request);
                }
                break;
            }
            if let Some(latest) = state.0.take() {
                request = latest;
            }
            drop(state);
            run(request);
        });
        Self {
            state,
            thread: Some(thread),
        }
    }
    /// Drain the most recent request and wait for its write before shutdown.
    pub(crate) fn finish(mut self) {
        {
            let mut state = self.state.0.lock().unwrap();
            state.1 = true;
            state.2 = true;
        }
        self.state.1.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
    pub(crate) fn submit(&self, request: T) {
        self.state.0.lock().unwrap().0 = Some(request);
        self.state.1.notify_one();
    }
}
impl<T> Drop for LatestWorker<T> {
    fn drop(&mut self) {
        self.state.0.lock().unwrap().1 = true;
        self.state.1.notify_one();
    }
}
#[derive(Clone, Serialize, PartialEq)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub directory: bool,
    pub depth: usize,
    pub expanded: bool,
}
pub use crate::git::{GitEntry, GitState};
pub enum Job {
    /// Browser directory, workspace boundary, expanded rows, tree mode.
    Browse(PathBuf, PathBuf, Vec<PathBuf>, bool),
    Git(crate::git::GitJob),
    /// List misspelled words with aspell, hunspell or enchant.
    Spell(String),
}
pub enum IoJob {
    Open(u64, PathBuf),
    PrepareSave(crate::saves::Preparation),
    FileOperation(crate::files::FileOperation),
    /// Reread a document's file, optionally decoding with a chosen encoding.
    Reload(u64, u64, PathBuf, Option<String>),
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
    Checkpoint(PathBuf, Workspace, u64),
    Flush(mpsc::SyncSender<()>),
}
pub struct SaveFailure {
    pub kind: crate::fsio::SaveErrorKind,
    pub message: String,
    pub snapshot: Option<Box<Document>>,
}
pub enum Reply {
    PreparedSave(
        Box<crate::saves::Preparation>,
        Result<Box<crate::saves::Prepared>, String>,
    ),
    DocumentSearch(
        crate::document_search::Request,
        Result<crate::document_search::Outcome, String>,
    ),
    Checkpoint(u64, Result<(), String>),
    Opened(u64, Result<Document, String>),
    FileOperation(crate::files::FileOperation, Result<(), String>),
    Reloaded(u64, u64, Result<Document, String>),
    OpenedDirectory(u64, PathBuf),
    Saved(u64, Result<Document, SaveFailure>),
    Highlighted(highlight::Response),
    Overview(crate::desktop::Overview, usize),
    Files(PathBuf, Result<Vec<Entry>, String>),
    Git(Result<GitState, String>),
    GitOperation(Result<String, String>),
    GitPreview(Result<crate::comparison::Preview, String>),
    Spelling(Result<Vec<String>, String>),
    Disk(Vec<(u64, crate::document::DiskChange)>),
    /// The workspace file index for a root.
    Index(PathBuf, Vec<String>),
    Filtered(u64, Vec<(usize, Vec<u32>)>),
    SearchHits(u64, Vec<crate::project::Hit>),
    SearchDone(u64, Result<crate::project::SearchReport, String>),
    /// Grammar-based symbols of a document.
    Outline(u64, Vec<crate::outline::Symbol>),
    /// Files rewritten by replace-in-files.
    Replaced(crate::picker::ReplaceReport),
    /// Files undo-replace-in-files wrote back.
    ReplaceUndone(crate::picker::RestoreReport),
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
    sender: mpsc::SyncSender<Reply>,
    events: crate::events::Events,
}
impl ReplySender {
    pub fn send(&self, reply: Reply) -> Result<(), ()> {
        self.sender.send(reply).map_err(|_| ())?;
        self.events.notify();
        Ok(())
    }
}
/// Submission never waits for a busy worker. Callers can report overload.
pub struct QueueSender<T>(mpsc::SyncSender<T>);
impl<T> QueueSender<T> {
    pub fn send(&self, job: T) -> Result<(), std::io::Error> {
        self.0.try_send(job).map_err(|error| match error {
            mpsc::TrySendError::Full(_) => std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "Service queue is full; retry when work finishes",
            ),
            mpsc::TrySendError::Disconnected(_) => {
                std::io::Error::new(std::io::ErrorKind::BrokenPipe, "Service worker stopped")
            }
        })
    }
}
pub struct JobSender {
    /// Status and diffs; a slow status never delays staging or commits.
    git_read: QueueSender<Job>,
    git_write: QueueSender<Job>,
    files: QueueSender<Job>,
}
impl JobSender {
    pub fn send(&self, job: Job) -> Result<(), std::io::Error> {
        use crate::git::GitJob;
        match job {
            Job::Git(GitJob::Run(..) | GitJob::Hunk(..)) => self.git_write.send(job),
            Job::Git(_) => self.git_read.send(job),
            _ => self.files.send(job),
        }
    }
}
pub struct Services {
    output: ReplySender,
    pub tx: JobSender,
    pub rx: Receiver<Reply>,
    pub io: QueueSender<IoJob>,
    pub highlight: mpsc::SyncSender<highlight::Request>,
    /// The newest requested highlight generation per document.
    pub highlight_latest: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<u64, u64>>>,
}
impl Services {
    pub fn new(events: crate::events::Events) -> Self {
        let (git_read, read_jobs) = mpsc::sync_channel(64);
        let (git_write, write_jobs) = mpsc::sync_channel(64);
        let (files, file_jobs) = mpsc::sync_channel(64);
        let tx = JobSender {
            git_read: QueueSender(git_read),
            git_write: QueueSender(git_write),
            files: QueueSender(files),
        };
        let (sender, rx) = mpsc::sync_channel(256);
        let output = ReplySender { sender, events };
        let output_root = output.clone();
        let (io, io_jobs) = mpsc::sync_channel(128);
        let io_output = output.clone();
        thread::spawn(move || {
            while let Ok(job) = io_jobs.recv() {
                let reply = match job {
                    IoJob::PrepareSave(request) => {
                        let result = request
                            .prepare()
                            .map(Box::new)
                            .map_err(|e| format!("{e:#}"));
                        Reply::PreparedSave(Box::new(request), result)
                    }
                    IoJob::FileOperation(operation) => {
                        let result = operation.run();
                        Reply::FileOperation(operation, result)
                    }
                    IoJob::Open(id, path) => {
                        if path.is_dir() {
                            Reply::OpenedDirectory(id, path)
                        } else {
                            Reply::Opened(id, Document::open(&path).map_err(|e| format!("{e:#}")))
                        }
                    }
                    IoJob::Reload(id, version, path, encoding) => Reply::Reloaded(
                        id,
                        version,
                        Document::open_with_encoding(&path, encoding.as_deref())
                            .map_err(|e| format!("{e:#}")),
                    ),
                    IoJob::Save(id, mut doc, destination, overwrite, options) => {
                        let prepared = doc.prepare_save(destination.as_deref(), overwrite);
                        let ready = prepared.is_ok();
                        let result = prepared.and_then(|()| doc.write_prepared(options));
                        Reply::Saved(
                            id,
                            match result {
                                Ok(()) => Ok(doc),
                                Err(e) => Err(SaveFailure {
                                    kind: crate::fsio::error_kind(&e),
                                    message: format!("{e:#}"),
                                    snapshot: ready.then(|| Box::new(doc)),
                                }),
                            },
                        )
                    }
                    IoJob::SaveElevated(id, mut doc, program) => {
                        let output = io_output.clone();
                        thread::spawn(move || {
                            let result = (|| -> anyhow::Result<()> {
                                let path = doc
                                    .path
                                    .clone()
                                    .ok_or_else(|| anyhow::anyhow!("Missing save destination"))?;
                                let bytes = doc.encoded()?;
                                crate::fsio::write_elevated(
                                    &path,
                                    &bytes,
                                    doc.disk.as_ref(),
                                    &program,
                                    false,
                                )?;
                                doc.mark_saved_bytes(&bytes);
                                Ok(())
                            })();
                            let _ = output.send(Reply::Saved(
                                id,
                                match result {
                                    Ok(()) => Ok(doc),
                                    Err(e) => Err(SaveFailure {
                                        kind: crate::fsio::SaveErrorKind::Other,
                                        message: format!("{e:#}"),
                                        snapshot: Some(Box::new(doc)),
                                    }),
                                },
                            ));
                        });
                        continue;
                    }
                    IoJob::Checkpoint(path, w, fingerprint) => Reply::Checkpoint(
                        fingerprint,
                        workspace::write_checkpoint(&path, &w).map_err(|e| format!("{e:#}")),
                    ),
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
                        Job::Browse(path, workspace, expanded, tree) => Reply::Files(
                            path.clone(),
                            crate::files::listing(&path, &workspace, &expanded, tree)
                                .map_err(|e| e.to_string()),
                        ),
                        Job::Spell(text) => Reply::Spelling(spell(&text)),
                        Job::Git(job) => match crate::git::handle(job) {
                            crate::git::GitReply::Status(state) => Reply::Git(state),
                            crate::git::GitReply::Operation(result) => Reply::GitOperation(result),
                            crate::git::GitReply::Preview(result) => Reply::GitPreview(result),
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
            io: QueueSender(io),
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
    // Run by absolute path: never a checker in the current folder.
    let (program, path, args) = checkers
        .iter()
        .find_map(|(program, args)| crate::fsio::which(program).map(|path| (program, path, args)))
        .ok_or("Install aspell, hunspell or enchant to check spelling")?;
    let mut child = crate::process::sanitize(&mut Command::new(path))
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
