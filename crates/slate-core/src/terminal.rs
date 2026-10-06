use anyhow::{Context, Result};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::{
    io::{Read, Write},
    path::Path,
    sync::{Arc, Mutex},
    thread,
};

#[derive(Clone, Debug, Serialize)]
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
pub struct TerminalSession {
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Box<dyn Child + Send + Sync>,
    parser: Arc<Mutex<vt100::Parser>>,
    pub title: String,
    size: (u16, u16),
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
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            let mut pending = Vec::new();
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
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
        })
    }
    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        let (rows, cols) = (rows.max(1), cols.max(1));
        if self.size == (rows, cols) {
            return Ok(());
        }
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
        let mut p = self.parser.lock().unwrap();
        let offset = p.screen().scrollback();
        p.screen_mut()
            .set_scrollback((offset as i32 + delta).max(0) as usize);
    }
    pub fn exited(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_some()
    }
    pub fn screen(&self) -> Screen {
        let p = self.parser.lock().unwrap();
        let s = p.screen();
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
            cursor: if s.hide_cursor() || s.scrollback() > 0 {
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
