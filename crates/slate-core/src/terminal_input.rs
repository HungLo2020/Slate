//! Nonblocking PTY input with budgets that include the write currently in flight.
use std::{
    io::{self, Write},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
};

pub(crate) const INPUT_BYTES: usize = 4 * 1024 * 1024;
const INPUT_MESSAGES: usize = 1024;

#[derive(Default)]
struct State {
    bytes: AtomicUsize,
    closed: AtomicBool,
    error: Mutex<Option<String>>,
}
struct Message {
    bytes: Vec<u8>,
    state: Arc<State>,
}
impl Drop for Message {
    fn drop(&mut self) {
        self.state
            .bytes
            .fetch_sub(self.bytes.len(), Ordering::AcqRel);
    }
}
#[derive(Clone)]
pub(crate) struct InputQueue {
    sender: mpsc::SyncSender<Message>,
    state: Arc<State>,
}
impl InputQueue {
    pub(crate) fn start(mut writer: impl Write + Send + 'static) -> Self {
        let (sender, receiver) = mpsc::sync_channel::<Message>(INPUT_MESSAGES);
        let state = Arc::new(State::default());
        let output = state.clone();
        std::thread::spawn(move || {
            for message in receiver {
                if output.closed.load(Ordering::Acquire) {
                    break;
                }
                if let Err(error) = writer
                    .write_all(&message.bytes)
                    .and_then(|_| writer.flush())
                {
                    *output.error.lock().unwrap() = Some(format!("Terminal input failed: {error}"));
                    break;
                }
            }
            output.closed.store(true, Ordering::Release);
        });
        Self { sender, state }
    }
    pub(crate) fn send(&self, bytes: &[u8]) -> io::Result<()> {
        if self.state.closed.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "The terminal has exited",
            ));
        }
        if bytes.is_empty() {
            return Ok(());
        }
        self.state
            .bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |queued| {
                queued
                    .checked_add(bytes.len())
                    .filter(|n| *n <= INPUT_BYTES)
            })
            .map_err(|_| Self::full())?;
        let message = Message {
            bytes: bytes.to_vec(),
            state: self.state.clone(),
        };
        self.sender.try_send(message).map_err(|error| match error {
            mpsc::TrySendError::Full(_) => Self::full(),
            mpsc::TrySendError::Disconnected(_) => {
                io::Error::new(io::ErrorKind::BrokenPipe, "The terminal has exited")
            }
        })
    }
    fn full() -> io::Error {
        io::Error::new(io::ErrorKind::WouldBlock,
            "Terminal input is full (4 MiB maximum); input was not sent. Wait for the program to read, then retry a smaller paste")
    }
    pub(crate) fn report(&self, error: impl ToString) {
        *self.state.error.lock().unwrap() = Some(error.to_string());
    }
    pub(crate) fn take_error(&self) -> Option<String> {
        self.state.error.lock().unwrap().take()
    }
    pub(crate) fn close(&self) {
        self.state.closed.store(true, Ordering::Release);
    }
}

#[cfg(test)]
impl InputQueue {
    pub(crate) fn queued_bytes(&self) -> usize {
        self.state.bytes.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    struct StalledWriter {
        entered: mpsc::SyncSender<()>,
        release: mpsc::Receiver<()>,
        output: Arc<Mutex<Vec<u8>>>,
        fail: bool,
    }
    impl Write for StalledWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let _ = self.entered.try_send(());
            // Disconnecting the release sender also releases the writer on a failed test.
            let _ = self.release.recv();
            if self.fail {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "fixture"));
            }
            self.output.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    type Stalled = (
        InputQueue,
        mpsc::Receiver<()>,
        mpsc::Sender<()>,
        Arc<Mutex<Vec<u8>>>,
    );
    fn stalled(fail: bool) -> Stalled {
        let (entered, seen) = mpsc::sync_channel(1);
        let (release, resume) = mpsc::channel();
        let output = Arc::new(Mutex::new(Vec::new()));
        let queue = InputQueue::start(StalledWriter {
            entered,
            release: resume,
            output: output.clone(),
            fail,
        });
        (queue, seen, release, output)
    }
    fn wait(check: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !check() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn byte_limit_includes_blocked_write_and_rejection_is_atomic() {
        let (queue, entered, release, output) = stalled(false);
        let bytes = vec![b'x'; INPUT_BYTES];
        queue.send(&bytes).unwrap();
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        let started = Instant::now();
        for _ in 0..100 {
            assert_eq!(
                queue.send(b"rejected").unwrap_err().kind(),
                io::ErrorKind::WouldBlock
            );
        }
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(queue.state.bytes.load(Ordering::Acquire), INPUT_BYTES);
        release.send(()).unwrap();
        wait(|| queue.state.bytes.load(Ordering::Acquire) == 0);
        queue.send(b"retry").unwrap();
        release.send(()).unwrap();
        wait(|| output.lock().unwrap().len() == INPUT_BYTES + 5);
        assert!(output.lock().unwrap().ends_with(b"retry"));
    }
    #[test]
    fn message_limit_preserves_order_and_failed_reservations_are_released() {
        let (queue, entered, release, output) = stalled(false);
        queue.send(b"first").unwrap();
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        for i in 0..INPUT_MESSAGES {
            queue.send(&i.to_le_bytes()).unwrap();
        }
        let charged = queue.state.bytes.load(Ordering::Acquire);
        assert_eq!(
            queue.send(b"extra").unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(queue.state.bytes.load(Ordering::Acquire), charged);
        drop(release);
        wait(|| queue.state.bytes.load(Ordering::Acquire) == 0);
        let expected: Vec<u8> = b"first"
            .iter()
            .copied()
            .chain((0..INPUT_MESSAGES).flat_map(usize::to_le_bytes))
            .collect();
        assert_eq!(*output.lock().unwrap(), expected);
    }
    #[test]
    fn writer_failure_and_close_release_pending_input() {
        for fail in [false, true] {
            let (queue, entered, release, _) = stalled(fail);
            queue.send(b"in flight").unwrap();
            entered.recv_timeout(Duration::from_secs(2)).unwrap();
            queue.send(b"queued").unwrap();
            if !fail {
                queue.close();
            }
            drop(release);
            wait(|| queue.state.bytes.load(Ordering::Acquire) == 0);
            assert_eq!(
                queue.send(b"after close").unwrap_err().kind(),
                io::ErrorKind::BrokenPipe
            );
            if fail {
                assert!(queue.take_error().unwrap().contains("fixture"));
            }
        }
    }
}
