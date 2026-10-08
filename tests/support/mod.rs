//! Drive the real `slate` executable in a pseudo-terminal and read its
//! screen through a VT emulator, so tests see what a user would see.
#![allow(dead_code)]
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub struct Tui {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    pub child: Box<dyn Child + Send + Sync>,
    parser: Arc<Mutex<vt100::Parser>>,
    raw: Arc<Mutex<Vec<u8>>>,
}

pub struct Env {
    pub dir: tempfile::TempDir,
}
impl Env {
    pub fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }
    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }
    pub fn config(&self, settings: &str) {
        let dir = self.path("config/slate");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("settings.toml"), settings).unwrap();
    }
    pub fn command(&self, program: &str) -> CommandBuilder {
        let mut command = CommandBuilder::new(program);
        command.cwd(self.dir.path());
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        command.env("SHELL", "/bin/sh");
        command.env("XDG_CONFIG_HOME", self.path("config"));
        command.env("XDG_STATE_HOME", self.path("state"));
        command.env_remove("NO_COLOR");
        command.env_remove("WAYLAND_DISPLAY");
        command.env_remove("DISPLAY");
        command
    }
    pub fn slate(&self, args: &[&str]) -> Tui {
        let mut command = self.command(env!("CARGO_BIN_EXE_slate"));
        command.args(args);
        Tui::spawn(command, 100, 30)
    }
}

impl Tui {
    pub fn spawn(command: CommandBuilder, cols: u16, rows: u16) -> Self {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let child = pair.slave.spawn_command(command).unwrap();
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().unwrap();
        let writer = pair.master.take_writer().unwrap();
        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));
        let raw = Arc::new(Mutex::new(Vec::new()));
        let (output, bytes) = (parser.clone(), raw.clone());
        std::thread::spawn(move || {
            let mut buffer = [0u8; 65536];
            while let Ok(n) = reader.read(&mut buffer) {
                if n == 0 {
                    break;
                }
                output.lock().unwrap().process(&buffer[..n]);
                bytes.lock().unwrap().extend_from_slice(&buffer[..n]);
            }
        });
        let mut tui = Self {
            master: pair.master,
            writer,
            child,
            parser,
            raw,
        };
        tui.wait_for("·");
        tui
    }
    pub fn send(&mut self, bytes: impl AsRef<[u8]>) {
        self.writer.write_all(bytes.as_ref()).unwrap();
        self.writer.flush().unwrap();
        std::thread::sleep(Duration::from_millis(120));
    }
    pub fn screen(&self) -> String {
        self.parser.lock().unwrap().screen().contents()
    }
    pub fn raw(&self) -> Vec<u8> {
        self.raw.lock().unwrap().clone()
    }
    pub fn pid(&self) -> u32 {
        self.child.process_id().unwrap()
    }
    pub fn wait_for(&mut self, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            if self.screen().contains(text) {
                return;
            }
            if let Ok(Some(status)) = self.child.try_wait() {
                panic!(
                    "slate exited ({status:?}) waiting for {text:?}:\n{}",
                    self.screen()
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("Timed out waiting for {text:?}:\n{}", self.screen());
    }
    /// Wait until the raw output after byte `from` contains `needle`.
    pub fn wait_raw(&mut self, from: usize, needle: &str) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while !String::from_utf8_lossy(&self.raw()[from.min(self.raw().len())..]).contains(needle) {
            assert!(Instant::now() < deadline, "No {needle:?} in the output");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    pub fn wait_gone(&mut self, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while self.screen().contains(text) {
            assert!(
                Instant::now() < deadline,
                "{text:?} stayed:\n{}",
                self.screen()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    /// Run a palette command (`F1`, then `:command`).
    pub fn command(&mut self, text: &str) {
        self.send(b"\x1bOP");
        self.send(format!(":{text}"));
        self.send(b"\r");
    }
    pub fn paste(&mut self, text: &str) {
        self.send(format!("\x1b[200~{text}\x1b[201~"));
    }
    pub fn exit_code(&mut self) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                return status.exit_code();
            }
            assert!(
                Instant::now() < deadline,
                "slate did not exit:\n{}",
                self.screen()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    pub fn resize(&self, cols: u16, rows: u16) {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        self.parser
            .lock()
            .unwrap()
            .screen_mut()
            .set_size(rows, cols);
    }
}
impl Drop for Tui {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn wait_file(path: &Path, expected: &[u8]) {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let current = std::fs::read(path).unwrap_or_default();
        if current == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{} contains {:?}, expected {:?}",
            path.display(),
            String::from_utf8_lossy(&current),
            String::from_utf8_lossy(expected)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
