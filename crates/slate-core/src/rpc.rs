//! Content-Length framed JSON over a child process's stdio, as used by both
//! the Language Server Protocol and the Debug Adapter Protocol. Reading and
//! writing happen on their own threads; the editor never blocks on a tool.
use serde_json::Value;
use std::{
    collections::VecDeque,
    io::{self, BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{Arc, Condvar, Mutex},
    thread,
    time::{Duration, Instant},
};

/// Messages larger than this end the connection.
const MESSAGE_LIMIT: usize = 16 * 1024 * 1024;
/// Longest header line accepted.
const HEADER_LIMIT: u64 = 8 * 1024;
/// Standard error kept for error reports.
const STDERR_TAIL: usize = 4096;
const QUEUE_BYTES: usize = 16 * 1024 * 1024;
const QUEUE_MESSAGES: usize = 1024;
/// A write to the child blocked this long: it no longer reads its input.
const STALLED: Duration = Duration::from_secs(10);

/// Messages waiting for the writer thread.
#[derive(Default)]
struct Outgoing {
    /// Framed messages, each with the key of `send_replacing`.
    queue: VecDeque<(Option<String>, Vec<u8>)>,
    /// Queued bytes, and those being written.
    bytes: usize,
    /// Since when the writer has been blocked writing to the child.
    writing: Option<Instant>,
    closed: bool,
}

pub struct Process {
    child: Child,
    outgoing: Arc<(Mutex<Outgoing>, Condvar)>,
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
        crate::process::sanitize(&mut cmd)
            .args(args)
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
        let outgoing = Arc::new((Mutex::new(Outgoing::default()), Condvar::new()));
        let writer = outgoing.clone();
        thread::spawn(move || {
            let (state, wake) = &*writer;
            loop {
                let bytes = {
                    let mut s = state.lock().unwrap();
                    while s.queue.is_empty() && !s.closed {
                        s = wake.wait(s).unwrap();
                    }
                    let Some((_, bytes)) = s.queue.pop_front() else {
                        break;
                    };
                    s.writing = Some(Instant::now());
                    bytes
                };
                let result = stdin.write_all(&bytes).and_then(|_| stdin.flush());
                let mut s = state.lock().unwrap();
                // Counted until written, like the queue.
                s.bytes -= bytes.len();
                s.writing = None;
                if result.is_err() {
                    s.closed = true;
                    s.queue.clear();
                    s.bytes = 0;
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
        Ok(Self { child, outgoing })
    }

    /// Queue `message`. False when it was not queued: the child is busy
    /// (its queue is full), or gone. A busy child is left running, unless
    /// it has stopped reading its input altogether.
    pub fn send(&self, message: &Value) -> bool {
        self.enqueue(None, message)
    }

    /// Queue `message`, replacing a message with the same `key` that has not
    /// been written yet, in its place. For messages that make earlier ones
    /// obsolete (a document's full text); the order of other messages is
    /// kept.
    pub fn send_replacing(&self, key: &str, message: &Value) -> bool {
        self.enqueue(Some(key), message)
    }

    fn enqueue(&self, key: Option<&str>, message: &Value) -> bool {
        let bytes = frame(message);
        let (state, wake) = &*self.outgoing;
        let mut s = state.lock().unwrap();
        if s.closed {
            return false;
        }
        let earlier = key.and_then(|key| {
            s.queue
                .iter()
                .position(|(queued, _)| queued.as_deref() == Some(key))
        });
        let freed = earlier.map_or(0, |i| s.queue[i].1.len());
        let fits = s.bytes - freed + bytes.len() <= QUEUE_BYTES
            && (earlier.is_some() || s.queue.len() < QUEUE_MESSAGES);
        if !fits {
            if s.writing.is_some_and(|since| since.elapsed() >= STALLED) {
                drop(s);
                self.terminate_stalled();
            }
            return false;
        }
        s.bytes = s.bytes - freed + bytes.len();
        match earlier {
            Some(i) => s.queue[i].1 = bytes,
            None => s.queue.push_back((key.map(str::to_owned), bytes)),
        }
        wake.notify_one();
        true
    }

    fn terminate_stalled(&self) {
        #[cfg(unix)]
        crate::process::kill_group(self.child.id(), libc::SIGKILL);
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
        let (state, wake) = &*self.outgoing;
        state.lock().unwrap().closed = true;
        wake.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

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

    #[cfg(unix)]
    #[test]
    fn a_busy_child_keeps_running_and_replaced_messages_keep_their_place() {
        let dir = tempfile::tempdir().unwrap();
        let mut process =
            Process::spawn(&["sleep".into(), "60".into()], dir.path(), |_| true, |_| {}).unwrap();
        // Larger than a pipe: the writer stays blocked writing it.
        let big = serde_json::json!({"text": "q".repeat(15 * 1024 * 1024)});
        assert!(process.send(&big));
        // However slowly the writer thread is scheduled, wait until it has
        // taken that message and is blocked writing it.
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let state = process.outgoing.0.lock().unwrap();
            if state.queue.is_empty() && state.writing.is_some() {
                break;
            }
            drop(state);
            assert!(Instant::now() < deadline, "the writer never started");
            thread::sleep(Duration::from_millis(5));
        }
        let text = |n: usize| serde_json::json!({"method": "didChange", "text": n});
        assert!(process.send_replacing("doc", &text(1)));
        assert!(process.send(&serde_json::json!({"method": "hover"})));
        for n in 2..50 {
            assert!(process.send_replacing("doc", &text(n)));
        }
        {
            let state = process.outgoing.0.lock().unwrap();
            let queued: Vec<Value> = state
                .queue
                .iter()
                .map(|(_, bytes)| read_message(&mut io::Cursor::new(bytes)).unwrap().unwrap())
                .collect();
            assert_eq!(queued, [text(49), serde_json::json!({"method": "hover"})]);
        }
        // Over the budget: refused, but the child is not killed.
        assert!(!process.send(&big));
        assert!(process.child.try_wait().unwrap().is_none());
    }
}
