//! Single-instance file opening for the graphical interface: a second
//! `slate-gui FILE` hands its files to the running window and exits, like
//! desktop editors. Directories and `--new-instance` always start a window.
use crate::cli::LaunchFile;
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Serialize, Deserialize)]
struct Request {
    files: Vec<(PathBuf, Option<usize>, Option<usize>)>,
}

/// Files handed over by other `slate-gui` invocations, waiting to be opened.
pub type Inbox = Arc<Mutex<Vec<LaunchFile>>>;

#[cfg(unix)]
fn socket_path() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute() && p.is_dir())
        .unwrap_or_else(std::env::temp_dir);
    let uid = unsafe { libc::geteuid() };
    base.join(format!("slate-gui-{uid}.sock"))
}

/// Send files to a running instance. Returns true when it accepted them.
#[cfg(unix)]
pub fn forward(files: &[LaunchFile]) -> bool {
    use std::io::{BufRead, BufReader, Write};
    let Ok(mut stream) = std::os::unix::net::UnixStream::connect(socket_path()) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(3)));
    let request = Request {
        files: files
            .iter()
            .map(|f| {
                let path = crate::document::absolute(&f.path).unwrap_or_else(|_| f.path.clone());
                (path, f.line, f.column)
            })
            .collect(),
    };
    let Ok(mut json) = serde_json::to_vec(&request) else {
        return false;
    };
    json.push(b'\n');
    if stream.write_all(&json).is_err() {
        return false;
    }
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply).is_ok() && reply.trim() == "ok"
}
#[cfg(not(unix))]
pub fn forward(_files: &[LaunchFile]) -> bool {
    false
}

/// Accept files from later invocations. `notify` wakes the event loop.
#[cfg(unix)]
pub fn listen(notify: impl Fn() + Send + 'static) -> Option<Inbox> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    let path = socket_path();
    // A leftover socket from a crashed instance refuses connections.
    if path.exists() && UnixStream::connect(&path).is_err() {
        let _ = std::fs::remove_file(&path);
    }
    let listener = UnixListener::bind(&path).ok()?;
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    let inbox: Inbox = Default::default();
    let queue = inbox.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut line = String::new();
            let mut reader = BufReader::new(&stream);
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let Ok(request) = serde_json::from_str::<Request>(&line) else {
                continue;
            };
            queue.lock().unwrap().extend(
                request
                    .files
                    .into_iter()
                    .map(|(path, line, column)| LaunchFile { path, line, column }),
            );
            notify();
            let _ = (&stream).write_all(b"ok\n");
        }
    });
    Some(inbox)
}
#[cfg(not(unix))]
pub fn listen(_notify: impl Fn() + Send + 'static) -> Option<Inbox> {
    None
}

/// Remove the socket when the primary instance exits.
pub fn release() {
    #[cfg(unix)]
    {
        let _ = std::fs::remove_file(socket_path());
    }
}

use crate::{App, Command};
impl App {
    pub fn attach_inbox(&mut self, inbox: Inbox) {
        self.inbox = Some(inbox);
    }
    /// Open files handed over by other invocations and ask to be raised.
    pub(crate) fn drain_inbox(&mut self) {
        let files = match &self.inbox {
            Some(inbox) => std::mem::take(&mut *inbox.lock().unwrap()),
            None => return,
        };
        if files.is_empty() {
            return;
        }
        for file in files {
            if file.path.is_dir() {
                continue;
            }
            if let (Some(line), true) = (file.line, file.path.exists()) {
                self.pending_positions.insert(
                    file.path.clone(),
                    (
                        line.saturating_sub(1),
                        crate::navigation::Column::Display(
                            file.column.unwrap_or(1).saturating_sub(1),
                        ),
                    ),
                );
            }
            self.dispatch(Command::Open { path: file.path });
        }
        self.frontend_requests.push("raise".into());
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_second_invocation_hands_files_to_the_first() {
        let runtime = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_RUNTIME_DIR", runtime.path());
        let woke = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = woke.clone();
        let inbox = listen(move || flag.store(true, std::sync::atomic::Ordering::SeqCst)).unwrap();
        let file = LaunchFile {
            path: runtime.path().join("a.txt"),
            line: Some(3),
            column: None,
        };
        assert!(forward(std::slice::from_ref(&file)));
        let received = inbox.lock().unwrap().clone();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].line, Some(3));
        assert!(woke.load(std::sync::atomic::Ordering::SeqCst));
        release();
        assert!(!forward(&[file]), "No instance after release");
    }
}
