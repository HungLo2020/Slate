//! Coalesced wakeups; workers never access application or frontend state.
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
#[cfg(unix)]
use std::{
    io::{Read, Write},
    os::fd::{AsRawFd, RawFd},
    os::unix::net::UnixStream,
};

#[derive(Clone)]
pub struct Events(Arc<Inner>);
type Waker = Box<dyn Fn() + Send + Sync>;
struct Inner {
    generation: Mutex<u64>,
    ready: Condvar,
    /// Frontends without an event descriptor are woken through a callback.
    waker: Mutex<Option<Waker>>,
    #[cfg(unix)]
    pipe: (Mutex<UnixStream>, Mutex<UnixStream>),
}
impl Default for Events {
    fn default() -> Self {
        #[cfg(unix)]
        let pipe = {
            let (read, write) = UnixStream::pair().expect("editor event pipe");
            read.set_nonblocking(true)
                .expect("nonblocking event reader");
            write
                .set_nonblocking(true)
                .expect("nonblocking event writer");
            (Mutex::new(read), Mutex::new(write))
        };
        Self(Arc::new(Inner {
            generation: Mutex::new(0),
            ready: Condvar::new(),
            waker: Mutex::new(None),
            #[cfg(unix)]
            pipe,
        }))
    }
}
impl Events {
    pub fn notify(&self) {
        *self.0.generation.lock().unwrap() += 1;
        self.0.ready.notify_all();
        if let Some(waker) = self.0.waker.lock().unwrap().as_ref() {
            waker();
        }
        #[cfg(unix)]
        {
            let _ = self.0.pipe.1.lock().unwrap().write(&[1]);
        }
    }
    pub fn set_waker(&self, waker: impl Fn() + Send + Sync + 'static) {
        *self.0.waker.lock().unwrap() = Some(Box::new(waker));
    }
    pub fn generation(&self) -> u64 {
        *self.0.generation.lock().unwrap()
    }
    pub fn wait(&self, observed: u64, timeout: Duration) {
        let generation = self.0.generation.lock().unwrap();
        let _ = self
            .0
            .ready
            .wait_timeout_while(generation, timeout, |g| *g == observed)
            .unwrap();
    }
    #[cfg(unix)]
    pub fn fd(&self) -> RawFd {
        self.0.pipe.0.lock().unwrap().as_raw_fd()
    }
    pub fn drain(&self) {
        #[cfg(unix)]
        {
            let mut bytes = [0; 4096];
            while self
                .0
                .pipe
                .0
                .lock()
                .unwrap()
                .read(&mut bytes)
                .is_ok_and(|n| n > 0)
            {}
        }
    }
}
