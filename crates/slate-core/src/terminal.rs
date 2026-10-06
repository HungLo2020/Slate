use anyhow::{Context, Result};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
};

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Cell {
    pub text: String,
    pub fg: String,
    pub bg: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub wide: bool,
    pub continuation: bool,
}
impl Cell {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            fg: "#d8dee9".into(),
            bg: "#20242c".into(),
            bold: false,
            italic: false,
            underline: false,
            wide: false,
            continuation: false,
        }
    }
}
#[derive(Clone, Serialize)]
pub struct Screen {
    pub cells: Vec<Vec<Cell>>,
    pub cursor: Option<(u16, u16)>,
    pub rows: u16,
    pub cols: u16,
}
#[derive(Default)]
pub struct Pointer<'a> {
    pub row: usize,
    pub col: usize,
    pub kind: &'a str,
    pub button: u8,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}
pub struct TerminalSession {
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Box<dyn Child + Send + Sync>,
    parser: Arc<Mutex<vt100::Parser>>,
    pub title: String,
    size: (u16, u16),
    initial_cwd: PathBuf,
    reported_cwd: Arc<Mutex<Option<PathBuf>>>,
    selection: Option<((u16, u16), (u16, u16))>,
    selection_screen: Option<vt100::Screen>,
}
impl TerminalSession {
    pub fn spawn(cwd: &Path, command: Option<&str>, rows: u16, cols: u16) -> Result<Self> {
        let pair = native_pty_system().openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let mut cmd = CommandBuilder::new(&shell);
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        if let Some(command) = command {
            cmd.args(["-c", command]);
        }
        let child = pair
            .slave
            .spawn_command(cmd)
            .context("Cannot spawn terminal shell")?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader()?;
        let writer = Arc::new(Mutex::new(pair.master.take_writer()?));
        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 5000)));
        let output = parser.clone();
        let responses = writer.clone();
        let reported_cwd = Arc::new(Mutex::new(None));
        let cwd_output = reported_cwd.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            let mut pending = Vec::new();
            let mut cwd_pending = Vec::new();
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                update_cwd(&mut cwd_pending, &buf[..n], &cwd_output);
                let mut p = output.lock().unwrap_or_else(|e| e.into_inner());
                p.process(&buf[..n]);
                // Respond to the cursor/device queries commonly used by interactive programs.
                pending.extend_from_slice(&buf[..n]);
                let mut replies = Vec::new();
                for query in [
                    b"\x1b[6n".as_slice(),
                    b"\x1b[c".as_slice(),
                    b"\x1b[0c".as_slice(),
                ] {
                    while let Some(i) = pending.windows(query.len()).position(|w| w == query) {
                        if query == b"\x1b[6n" {
                            let (r, c) = p.screen().cursor_position();
                            replies
                                .extend_from_slice(format!("\x1b[{};{}R", r + 1, c + 1).as_bytes());
                        } else {
                            replies.extend_from_slice(b"\x1b[?1;2c");
                        }
                        pending.drain(i..i + query.len());
                    }
                }
                if pending.len() > 16 {
                    pending.drain(..pending.len() - 16);
                }
                drop(p);
                if !replies.is_empty() {
                    if let Ok(mut w) = responses.lock() {
                        let _ = w.write_all(&replies);
                        let _ = w.flush();
                    }
                }
            }
        });
        Ok(Self {
            master: pair.master,
            writer,
            child,
            parser,
            title: shell,
            size: (rows, cols),
            initial_cwd: cwd.to_path_buf(),
            reported_cwd,
            selection: None,
            selection_screen: None,
        })
    }
    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        let (rows, cols) = (rows.max(1), cols.max(1));
        if self.size == (rows, cols) {
            return Ok(());
        }
        self.selection = None;
        self.selection_screen = None;
        self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        self.parser
            .lock()
            .unwrap()
            .screen_mut()
            .set_size(rows, cols);
        self.size = (rows, cols);
        Ok(())
    }
    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.selection = None;
        self.selection_screen = None;
        self.parser.lock().unwrap().screen_mut().set_scrollback(0);
        let mut writer = self.writer.lock().unwrap();
        writer.write_all(bytes)?;
        writer.flush()?;
        Ok(())
    }
    pub fn paste(&mut self, text: &str) -> Result<()> {
        let bracketed = self.parser.lock().unwrap().screen().bracketed_paste();
        if bracketed {
            self.write(b"\x1b[200~")?;
        }
        self.write(text.as_bytes())?;
        if bracketed {
            self.write(b"\x1b[201~")?;
        }
        Ok(())
    }
    pub fn application_cursor(&self) -> bool {
        self.parser.lock().unwrap().screen().application_cursor()
    }
    pub fn scroll(&mut self, delta: i32) {
        self.selection = None;
        self.selection_screen = None;
        let mut p = self.parser.lock().unwrap();
        let offset = p.screen().scrollback();
        p.screen_mut()
            .set_scrollback((offset as i32 + delta).max(0) as usize);
    }
    pub fn cwd(&self) -> PathBuf {
        #[cfg(target_os = "linux")]
        if let Some(pid) = self.child.process_id() {
            if let Ok(path) = std::fs::read_link(format!("/proc/{pid}/cwd")) {
                return path;
            }
        }
        self.reported_cwd
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| self.initial_cwd.clone())
    }
    pub fn select(&mut self, row: usize, col: usize, extend: bool) {
        let position = (
            row.min(self.size.0.saturating_sub(1) as usize) as u16,
            col.min(self.size.1 as usize) as u16,
        );
        if !extend || self.selection.is_none() {
            self.selection = Some((position, position));
            self.selection_screen = Some(self.parser.lock().unwrap().screen().clone());
        } else if let Some((anchor, _)) = self.selection {
            self.selection = Some((anchor, position));
        }
    }
    pub fn selection_text(&self) -> String {
        match (&self.selection_screen, self.selection) {
            (Some(screen), Some((a, b))) => {
                let (start, end) = if a <= b { (a, b) } else { (b, a) };
                screen.contents_between(start.0, start.1, end.0, end.1)
            }
            _ => String::new(),
        }
    }
    pub fn pointer(&mut self, event: Pointer<'_>) -> Result<()> {
        let Pointer {
            row,
            col,
            kind,
            button,
            shift,
            ctrl,
            alt,
        } = event;
        use vt100::{MouseProtocolEncoding as Encoding, MouseProtocolMode as Mode};
        let (mode, encoding) = {
            let p = self.parser.lock().unwrap();
            (
                p.screen().mouse_protocol_mode(),
                p.screen().mouse_protocol_encoding(),
            )
        };
        if shift || mode == Mode::None {
            match kind {
                "press" if button == 0 => self.select(row, col, false),
                "drag" if button == 0 => self.select(row, col, true),
                "wheel_up" => self.scroll(3),
                "wheel_down" => self.scroll(-3),
                _ => {}
            }
            return Ok(());
        }
        let release = kind == "release";
        if (release && mode == Mode::Press)
            || (kind == "drag" && !matches!(mode, Mode::ButtonMotion | Mode::AnyMotion))
            || (kind == "move" && mode != Mode::AnyMotion)
        {
            return Ok(());
        }
        let mut code = match kind {
            "wheel_up" => 64,
            "wheel_down" => 65,
            "press" | "release" | "drag" | "move" => button.min(3) as u16,
            _ => return Ok(()),
        };
        if kind == "drag" || kind == "move" {
            code += 32;
        }
        code += u16::from(shift) * 4 + u16::from(alt) * 8 + u16::from(ctrl) * 16;
        let x = col.min(self.size.1.saturating_sub(1) as usize) + 1;
        let y = row.min(self.size.0.saturating_sub(1) as usize) + 1;
        let bytes = match encoding {
            Encoding::Sgr => {
                format!("\x1b[<{code};{x};{y}{}", if release { 'm' } else { 'M' }).into_bytes()
            }
            Encoding::Default => vec![
                27,
                b'[',
                b'M',
                (if release { 3 } else { code }) as u8 + 32,
                x.min(223) as u8 + 32,
                y.min(223) as u8 + 32,
            ],
            Encoding::Utf8 => format!(
                "\x1b[M{}{}{}",
                char::from_u32(u32::from(if release { 3 } else { code }) + 32).unwrap(),
                char::from_u32(x.min(2015) as u32 + 32).unwrap(),
                char::from_u32(y.min(2015) as u32 + 32).unwrap()
            )
            .into_bytes(),
        };
        self.write(&bytes)
    }
    pub fn exited(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_some()
    }
    pub fn screen(&self) -> Screen {
        let p = self.parser.lock().unwrap();
        let s = self.selection_screen.as_ref().unwrap_or_else(|| p.screen());
        let (rows, cols) = s.size();
        let mut cells = Vec::with_capacity(rows as usize);
        for row in 0..rows {
            let mut line = Vec::with_capacity(cols as usize);
            for col in 0..cols {
                let c = s.cell(row, col).unwrap();
                let mut fg = color(c.fgcolor(), false);
                let mut bg = color(c.bgcolor(), true);
                if c.inverse() {
                    std::mem::swap(&mut fg, &mut bg);
                }
                if let Some((a, b)) = self.selection {
                    let (start, end) = if a <= b { (a, b) } else { (b, a) };
                    if (row, col) >= start && (row, col) < end {
                        bg = "#425b78".into();
                    }
                }
                line.push(Cell {
                    text: c.contents().to_string(),
                    fg,
                    bg,
                    bold: c.bold(),
                    italic: c.italic(),
                    underline: c.underline(),
                    wide: c.is_wide(),
                    continuation: c.is_wide_continuation(),
                });
            }
            cells.push(line);
        }
        Screen {
            cells,
            cursor: if s.hide_cursor() || s.scrollback() > 0 || self.selection_screen.is_some() {
                None
            } else {
                Some(s.cursor_position())
            },
            rows,
            cols,
        }
    }
}
impl Drop for TerminalSession {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.child.process_id() {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGHUP);
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn color(c: vt100::Color, background: bool) -> String {
    match c {
        vt100::Color::Default => if background { "#20242c" } else { "#d8dee9" }.into(),
        vt100::Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        vt100::Color::Idx(i) => {
            const ANSI: [&str; 16] = [
                "#20242c", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#88c0d0",
                "#e5e9f0", "#4c566a", "#d08770", "#b8d7a3", "#f0dcab", "#a3c5e5", "#c9acd7",
                "#a3d8e0", "#ffffff",
            ];
            if i < 16 {
                return ANSI[i as usize].into();
            }
            if i >= 232 {
                let v = 8 + (i - 232) * 10;
                return format!("#{v:02x}{v:02x}{v:02x}");
            }
            let i = i - 16;
            let levels = [0, 95, 135, 175, 215, 255];
            format!(
                "#{:02x}{:02x}{:02x}",
                levels[(i / 36) as usize],
                levels[((i / 6) % 6) as usize],
                levels[(i % 6) as usize]
            )
        }
    }
}

// OSC 7 shell integration supplements /proc on systems that restrict process inspection.
fn update_cwd(pending: &mut Vec<u8>, bytes: &[u8], cwd: &Arc<Mutex<Option<PathBuf>>>) {
    pending.extend_from_slice(bytes);
    while let Some(start) = pending.windows(4).position(|w| w == b"\x1b]7;") {
        let body = start + 4;
        let end = pending[body..]
            .iter()
            .position(|b| *b == 7)
            .map(|i| (body + i, 1))
            .or_else(|| {
                pending[body..]
                    .windows(2)
                    .position(|w| w == b"\x1b\\")
                    .map(|i| (body + i, 2))
            });
        let Some((end, terminator)) = end else {
            if pending.len() > 8192 {
                pending.clear();
            } else if start > 0 {
                pending.drain(..start);
            }
            return;
        };
        if let Ok(url) = std::str::from_utf8(&pending[body..end]) {
            if let Some(location) = url.strip_prefix("file://") {
                if let Some(slash) = location.find('/') {
                    let host = &location[..slash];
                    if host.is_empty()
                        || host == "localhost"
                        || std::env::var("HOSTNAME").is_ok_and(|local| local == host)
                    {
                        let encoded = &location.as_bytes()[slash..];
                        let mut decoded = Vec::new();
                        let mut i = 0;
                        while i < encoded.len() {
                            if encoded[i] == b'%' && i + 2 < encoded.len() {
                                if let (Some(a), Some(b)) = (
                                    (encoded[i + 1] as char).to_digit(16),
                                    (encoded[i + 2] as char).to_digit(16),
                                ) {
                                    decoded.push((a * 16 + b) as u8);
                                    i += 3;
                                    continue;
                                }
                            }
                            decoded.push(encoded[i]);
                            i += 1;
                        }
                        if let Ok(path) = String::from_utf8(decoded) {
                            let path = PathBuf::from(path);
                            if path.is_absolute() && path.is_dir() {
                                *cwd.lock().unwrap() = Some(path);
                            }
                        }
                    }
                }
            }
        }
        pending.drain(..end + terminator);
    }
    if pending.len() > 4 {
        pending.drain(..pending.len() - 4);
    }
}
