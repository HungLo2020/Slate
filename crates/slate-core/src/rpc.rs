//! Content-Length framed JSON over a child process's stdio, as used by both
//! the Language Server Protocol and the Debug Adapter Protocol. Reading and
//! writing happen on their own threads; the editor never blocks on a tool.
use serde_json::Value;
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
};

/// Messages larger than this end the connection.
const MESSAGE_LIMIT: usize = 16 * 1024 * 1024;
/// Longest header line accepted.
const HEADER_LIMIT: u64 = 8 * 1024;
/// Standard error kept for error reports.
const STDERR_TAIL: usize = 4096;
const QUEUE_BYTES: usize = 16 * 1024 * 1024;

pub struct Process {
    child: Child,
    input: mpsc::SyncSender<Vec<u8>>,
    queued_bytes: Arc<AtomicUsize>,
}

pub fn frame(message: &Value) -> Vec<u8> {
    let body = serde_json::to_vec(message).unwrap_or_default();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend(body);
    out
}

/// Read one framed message; `None` at end of stream.
pub fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut length = None;
    loop {
        // Header lines are short; a peer that never sends a newline must
        // not grow this without bound.
        let mut line = String::new();
        if reader.take(HEADER_LIMIT).read_line(&mut line)? == 0 {
            return Ok(None);
        }
        if !line.ends_with('\n') && line.len() as u64 >= HEADER_LIMIT {
            return Err(io::Error::other("header line too long"));
        }
        let line = line.trim_end();
        if line.is_empty() {
            if length.is_some() {
                break;
            }
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                length = value.trim().parse::<usize>().ok();
            }
        }
    }
    let length = length.unwrap_or(0);
    if length > MESSAGE_LIMIT {
        return Err(io::Error::other("message too large"));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

impl Process {
    /// Start `command` in `cwd`. `on_message` runs on the reader thread for
    /// each message; `on_exit` once the output ends, with a reason.
    pub fn spawn(
        command: &[String],
        cwd: &Path,
        on_message: impl Fn(Value) -> bool + Send + 'static,
        on_exit: impl FnOnce(String) + Send + 'static,
    ) -> io::Result<Self> {
        let (program, args) = command
            .split_first()
            .ok_or_else(|| io::Error::other("empty command"))?;
        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Its own process group, so stopping it stops its helpers too.
            cmd.process_group(0);
        }
        let mut child = cmd.spawn()?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("no stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("no stdout"))?;
        let mut stderr_pipe = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("no stderr"))?;
        let (input, queued) = mpsc::sync_channel::<Vec<u8>>(128);
        let queued_bytes = Arc::new(AtomicUsize::new(0));
        let budget = queued_bytes.clone();
        thread::spawn(move || {
            for bytes in queued {
                let result = stdin.write_all(&bytes).and_then(|_| stdin.flush());
                budget.fetch_sub(bytes.len(), Ordering::Relaxed);
                if result.is_err() {
                    break;
                }
            }
        });
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let tail = stderr.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = stderr_pipe.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let mut t = tail.lock().unwrap();
                t.extend_from_slice(&buf[..n]);
                let excess = t.len().saturating_sub(STDERR_TAIL);
                t.drain(..excess);
            }
        });
        let tail = stderr;
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let reason = loop {
                match read_message(&mut reader) {
                    Ok(Some(message)) => {
                        if !on_message(message) {
                            break "stopped".to_string();
                        }
                    }
                    Ok(None) => break "exited".to_string(),
                    Err(e) => break e.to_string(),
                }
            };
            // Give standard error a moment to arrive for the report.
            thread::sleep(std::time::Duration::from_millis(50));
            let stderr = String::from_utf8_lossy(&tail.lock().unwrap())
                .trim()
                .to_string();
            let last = stderr.lines().last().unwrap_or_default().to_string();
            on_exit(if last.is_empty() {
                reason
            } else {
                format!("{reason}: {last}")
            });
        });
        Ok(Self {
            child,
            input,
            queued_bytes,
        })
    }

    pub fn send(&self, message: &Value) -> bool {
        let bytes = frame(message);
        let size = bytes.len();
        if self
            .queued_bytes
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                n.checked_add(size).filter(|n| *n <= QUEUE_BYTES)
            })
            .is_err()
        {
            self.terminate_overloaded();
            return false;
        }
        if self.input.try_send(bytes).is_err() {
            self.queued_bytes.fetch_sub(size, Ordering::Relaxed);
            self.terminate_overloaded();
            return false;
        }
        true
    }

    fn terminate_overloaded(&self) {
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
    }

    /// Stop the process group: give it `grace` to exit, then force it.
    pub fn stop(&mut self, grace: std::time::Duration) {
        crate::process::stop_group(&mut self.child, grace);
    }

    /// Stop the process (and its process group) now.
    pub fn kill(&mut self) {
        crate::process::stop_group(&mut self.child, std::time::Duration::from_millis(50));
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_and_tolerate_extra_headers() {
        let message = serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": "ünï"});
        let mut bytes = frame(&message);
        let mut extra = b"Content-Type: application/vscode-jsonrpc; charset=utf-8\r\n".to_vec();
        extra.extend(frame(&message));
        bytes.extend(extra);
        let mut reader = io::Cursor::new(bytes);
        assert_eq!(read_message(&mut reader).unwrap(), Some(message.clone()));
        assert_eq!(read_message(&mut reader).unwrap(), Some(message));
        assert_eq!(read_message(&mut reader).unwrap(), None);
    }

    #[test]
    fn a_child_process_echoes_messages() {
        let (tx, rx) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        // `cat` echoes every frame back.
        let process = Process::spawn(
            &["cat".into()],
            Path::new("/"),
            move |m| tx.send(m).is_ok(),
            move |reason| {
                let _ = done_tx.send(reason);
            },
        )
        .unwrap();
        let message = serde_json::json!({"method": "ping", "params": [1, 2]});
        assert!(process.send(&message));
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap(),
            message
        );
        drop(process);
        assert!(done.recv_timeout(std::time::Duration::from_secs(5)).is_ok());
    }
    #[cfg(unix)]
    #[test]
    fn a_child_that_stops_reading_cannot_grow_the_outgoing_queue() {
        let dir = tempfile::tempdir().unwrap();
        let process = Process::spawn(
            &[
                "python3".into(),
                "-c".into(),
                "import time; time.sleep(60)".into(),
            ],
            dir.path(),
            |_| true,
            |_| {},
        )
        .unwrap();
        let message = serde_json::json!({"text": "q".repeat(8 * 1024 * 1024)});
        assert!(process.send(&message));
        assert!(
            !process.send(&message),
            "A blocked child accepted more than the byte budget"
        );
    }
}
