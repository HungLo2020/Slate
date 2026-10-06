use serde::Serialize;
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::mpsc::{self, Receiver, Sender},
    thread,
};
#[derive(Clone, Serialize)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub directory: bool,
}
#[derive(Clone, Serialize)]
pub struct GitEntry {
    pub status: String,
    pub path: String,
}
pub enum Job {
    Browse(PathBuf),
    Git(PathBuf, Vec<String>, bool),
}
pub enum Reply {
    Files(PathBuf, Result<Vec<Entry>, String>),
    Git(Result<Vec<GitEntry>, String>),
    Output(String),
    Message(String),
    Error(String),
}
pub struct Services {
    pub tx: Sender<Job>,
    pub rx: Receiver<Reply>,
}
impl Services {
    pub fn new() -> Self {
        let (tx, jobs) = mpsc::channel();
        let (output, rx) = mpsc::channel();
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
                                b.directory
                                    .cmp(&a.directory)
                                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                            });
                            list.extend(children);
                            Ok(list)
                        })();
                        Reply::Files(path, entries.map_err(|e| e.to_string()))
                    }
                    Job::Git(root, args, diff) => {
                        let top = Command::new("git")
                            .arg("-C")
                            .arg(&root)
                            .args(["rev-parse", "--show-toplevel"])
                            .output()
                            .ok()
                            .filter(|r| r.status.success())
                            .map(|r| PathBuf::from(String::from_utf8_lossy(&r.stdout).trim()));
                        let root = top.unwrap_or(root);
                        let mut args = args;
                        if args.first().map(String::as_str) == Some("reset")
                            && !Command::new("git")
                                .arg("-C")
                                .arg(&root)
                                .args(["rev-parse", "--verify", "HEAD"])
                                .output()
                                .map(|r| r.status.success())
                                .unwrap_or(false)
                        {
                            args = vec![
                                "rm".into(),
                                "--cached".into(),
                                "--".into(),
                                args.last().cloned().unwrap_or_default(),
                            ];
                        }
                        let status_query = args.first().map(String::as_str) == Some("status");
                        let result = Command::new("git")
                            .arg("-C")
                            .arg(&root)
                            .args(&args)
                            .env("GIT_TERMINAL_PROMPT", "0")
                            .output();
                        match result {
                            Ok(result) if result.status.success() => {
                                if diff {
                                    Reply::Output(
                                        String::from_utf8_lossy(&result.stdout).into_owned(),
                                    )
                                } else if !status_query {
                                    Reply::Message("Git operation completed".into())
                                } else {
                                    // NUL delimiters preserve spaces and newlines; renamed entries carry a second path.
                                    let data = String::from_utf8_lossy(&result.stdout);
                                    let mut records = data.split('\0');
                                    let mut entries = vec![];
                                    while let Some(record) = records.next() {
                                        if record.len() < 3 {
                                            continue;
                                        }
                                        let status = record[..2].to_string();
                                        let path = record[3..].to_string();
                                        if status.contains('R') || status.contains('C') {
                                            records.next();
                                        }
                                        entries.push(GitEntry { status, path });
                                    }
                                    Reply::Git(Ok(entries))
                                }
                            }
                            Ok(result)
                                if status_query
                                    && String::from_utf8_lossy(&result.stderr)
                                        .contains("not a git repository") =>
                            {
                                Reply::Git(Ok(Vec::new()))
                            }
                            Ok(result) => Reply::Error(
                                String::from_utf8_lossy(&result.stderr).trim().to_string(),
                            ),
                            Err(e) => Reply::Error(e.to_string()),
                        }
                    }
                };
                if output.send(reply).is_err() {
                    break;
                }
            }
        });
        Self { tx, rx }
    }
}
