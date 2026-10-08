use anyhow::{Context, Result};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
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
/// Default lines of history kept per terminal.
pub const DEFAULT_SCROLLBACK: usize = 5000;

/// Side effects of terminal output that are not drawn on the screen. The
/// parser calls these while processing output, so replies to queries see
/// the screen exactly as it was when the query arrived.
#[derive(Default)]
struct Hooks {
    replies: Vec<u8>,
    title: Option<String>,
    clipboard: Vec<String>,
    cwd: Option<PathBuf>,
}
impl vt100::Callbacks for Hooks {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        let title: String = String::from_utf8_lossy(title)
            .chars()
            .filter(|c| !c.is_control())
            .take(120)
            .collect();
        self.title = Some(title.trim().to_string());
    }
    fn copy_to_clipboard(&mut self, _: &mut vt100::Screen, _ty: &[u8], data: &[u8]) {
        // Programs (tmux, vim, ssh sessions) copy with OSC 52. Reading the
        // clipboard back is never allowed.
        if data.len() <= 4 * 1024 * 1024 {
            if let Some(text) = decode_base64(data).and_then(|b| String::from_utf8(b).ok()) {
                self.clipboard.push(text);
            }
        }
    }
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        _i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        let first = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        match (i1, c, first) {
            // Cursor position report.
            (None, 'n', 6) => {
                let (r, c) = screen.cursor_position();
                self.replies
                    .extend_from_slice(format!("\x1b[{};{}R", r + 1, c + 1).as_bytes());
            }
            // Device status: OK.
            (None, 'n', 5) => self.replies.extend_from_slice(b"\x1b[0n"),
            // Primary device attributes: VT100 with advanced video.
            (None, 'c', 0) => self.replies.extend_from_slice(b"\x1b[?1;2c"),
            // Secondary device attributes.
            (Some(b'>'), 'c', 0) => self.replies.extend_from_slice(b"\x1b[>0;10;1c"),
            _ => {}
        }
    }
    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        // OSC 7 shell integration supplements /proc on systems that restrict
        // process inspection.
        if params.first() == Some(&b"7".as_slice()) {
            let url = params[1..].join(&b';');
            if let Some(path) = std::str::from_utf8(&url).ok().and_then(cwd_from_url) {
                self.cwd = Some(path);
            }
        }
    }
}
type Parser = vt100::Parser<Hooks>;

pub struct TerminalSession {
    master: Box<dyn MasterPty + Send>,
    /// Bytes for the shell, written by a dedicated thread so a slow or
    /// stalled program never blocks the interface.
    input: mpsc::Sender<Vec<u8>>,
    child: Box<dyn Child + Send + Sync>,
    parser: Arc<Mutex<Parser>>,
    revision: Arc<AtomicU64>,
    events: Arc<Mutex<Option<crate::events::Events>>>,
    finished: Arc<AtomicBool>,
    /// The shell's exit code and when it was seen.
    exit: Option<(u32, std::time::Instant)>,
    pub title: String,
    size: (u16, u16),
    initial_cwd: PathBuf,
    selection: Option<((u16, u16), (u16, u16))>,
    selection_screen: Option<vt100::Screen>,
}
impl TerminalSession {
    pub fn spawn(cwd: &Path, command: Option<&str>, rows: u16, cols: u16) -> Result<Self> {
        Self::spawn_with(cwd, command, rows, cols, DEFAULT_SCROLLBACK)
    }
    pub fn spawn_with(
        cwd: &Path,
        command: Option<&str>,
        rows: u16,
        cols: u16,
        scrollback: usize,
    ) -> Result<Self> {
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
        let mut writer = pair.master.take_writer()?;
        let (input, queued) = mpsc::channel::<Vec<u8>>();
        thread::spawn(move || {
            for bytes in queued {
                if writer
                    .write_all(&bytes)
                    .and_then(|_| writer.flush())
                    .is_err()
                {
                    break;
                }
            }
        });
        let parser = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(
            rows,
            cols,
            scrollback,
            Hooks::default(),
        )));
        let output = parser.clone();
        let responses = input.clone();
        let revision = Arc::new(AtomicU64::new(1));
        let output_revision = revision.clone();
        let events = Arc::new(Mutex::new(None::<crate::events::Events>));
        let output_events = events.clone();
        let finished = Arc::new(AtomicBool::new(false));
        let output_finished = finished.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let replies = {
                    let mut p = output.lock().unwrap_or_else(|e| e.into_inner());
                    p.process(&buf[..n]);
                    std::mem::take(&mut p.callbacks_mut().replies)
                };
                output_revision.fetch_add(1, Ordering::Release);
                if !replies.is_empty() {
                    let _ = responses.send(replies);
                }
                if let Some(events) = output_events.lock().unwrap().as_ref() {
                    events.notify();
                }
            }
            output_finished.store(true, Ordering::Release);
            output_revision.fetch_add(1, Ordering::Release);
            if let Some(events) = output_events.lock().unwrap().as_ref() {
                events.notify();
            }
        });
        Ok(Self {
            revision,
            events,
            finished,
            exit: None,
            master: pair.master,
            input,
            child,
            parser,
            title: shell,
            size: (rows, cols),
            initial_cwd: cwd.to_path_buf(),
            selection: None,
            selection_screen: None,
        })
    }
    /// The title set by the running program (OSC 0/2), if any.
    pub fn program_title(&self) -> Option<String> {
        self.parser
            .lock()
            .unwrap()
            .callbacks()
            .title
            .clone()
            .filter(|t| !t.is_empty())
    }
    /// Text copied by programs with OSC 52 since the last call.
    pub fn take_clipboard(&mut self) -> Option<String> {
        let mut p = self.parser.lock().unwrap();
        std::mem::take(&mut p.callbacks_mut().clipboard).pop()
    }
    /// The exit code once the shell has exited and its output is drained.
    pub fn exit_code(&mut self) -> Option<u32> {
        // The shell is reaped as soon as it exits (no zombie). Its last
        // output is waited for briefly; a background process that keeps the
        // terminal open does not keep a dead tab around.
        if self.exit.is_none() {
            let status = self.child.try_wait().ok().flatten()?;
            self.exit = Some((status.exit_code(), std::time::Instant::now()));
        }
        let (code, at) = self.exit?;
        (self.finished.load(Ordering::Acquire)
            || at.elapsed() > std::time::Duration::from_millis(300))
        .then_some(code)
    }
    pub fn set_events(&mut self, events: crate::events::Events) {
        *self.events.lock().unwrap() = Some(events);
    }
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }
    fn changed(&self) {
        self.revision.fetch_add(1, Ordering::Release);
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
        self.changed();
        Ok(())
    }
    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.selection = None;
        self.selection_screen = None;
        self.parser.lock().unwrap().screen_mut().set_scrollback(0);
        self.changed();
        self.input
            .send(bytes.to_vec())
            .map_err(|_| anyhow::anyhow!("The terminal has exited"))
    }
    pub fn paste(&mut self, text: &str) -> Result<()> {
        let bracketed = self.parser.lock().unwrap().screen().bracketed_paste();
        if bracketed {
            self.write(b"\x1b[200~")?;
        }
        // Control characters in pasted text could end bracketed paste early
        // (ESC [201~) and run what follows; keep text, tabs and line breaks.
        self.write(paste_text(text).as_bytes())?;
        if bracketed {
            self.write(b"\x1b[201~")?;
        }
        Ok(())
    }
    pub fn application_cursor(&self) -> bool {
        self.parser.lock().unwrap().screen().application_cursor()
    }
    pub fn mouse_motion(&self) -> bool {
        self.parser.lock().unwrap().screen().mouse_protocol_mode()
            == vt100::MouseProtocolMode::AnyMotion
    }
    pub fn scroll(&mut self, delta: i32) {
        self.changed();
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
        self.parser
            .lock()
            .unwrap()
            .callbacks()
            .cwd
            .clone()
            .unwrap_or_else(|| self.initial_cwd.clone())
    }
    pub fn select(&mut self, row: usize, col: usize, extend: bool) {
        self.changed();
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
    /// Lines of history currently kept above the screen.
    pub fn scrollback_len(&self) -> usize {
        let mut p = self.parser.lock().unwrap();
        let current = p.screen().scrollback();
        p.screen_mut().set_scrollback(usize::MAX);
        let len = p.screen().scrollback();
        p.screen_mut().set_scrollback(current);
        len
    }
    pub fn screen(&self) -> Screen {
        self.screen_with("#d8dee9", "#20242c")
    }
    /// The screen with the default foreground/background drawn in the
    /// editor theme's colours, so terminals follow light and dark themes.
    pub fn screen_with(&self, default_fg: &str, default_bg: &str) -> Screen {
        let p = self.parser.lock().unwrap();
        let s = self.selection_screen.as_ref().unwrap_or_else(|| p.screen());
        let (rows, cols) = s.size();
        let mut cells = Vec::with_capacity(rows as usize);
        for row in 0..rows {
            let mut line = Vec::with_capacity(cols as usize);
            for col in 0..cols {
                let c = s.cell(row, col).unwrap();
                let mut fg = match c.fgcolor() {
                    vt100::Color::Default => default_fg.to_string(),
                    other => color(other, false),
                };
                let mut bg = match c.bgcolor() {
                    vt100::Color::Default => default_bg.to_string(),
                    other => color(other, true),
                };
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

/// Pasted text without control characters other than tab and line breaks.
pub fn paste_text(text: &str) -> String {
    text.chars()
        .filter(|c| matches!(c, '\t' | '\n' | '\r') || !c.is_control())
        .collect()
}

/// The local directory named by an OSC 7 `file://host/path` URL.
fn cwd_from_url(url: &str) -> Option<PathBuf> {
    let location = url.strip_prefix("file://")?;
    let slash = location.find('/')?;
    let host = &location[..slash];
    if !(host.is_empty()
        || host == "localhost"
        || std::env::var("HOSTNAME").is_ok_and(|local| local == host))
    {
        return None;
    }
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
    let path = PathBuf::from(String::from_utf8(decoded).ok()?);
    (path.is_absolute() && path.is_dir()).then_some(path)
}

fn decode_base64(data: &[u8]) -> Option<Vec<u8>> {
    let value = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let data: Vec<u8> = data
        .iter()
        .copied()
        .filter(|c| !c.is_ascii_whitespace())
        .collect();
    let trimmed = data
        .strip_suffix(b"==")
        .or_else(|| data.strip_suffix(b"="))
        .unwrap_or(&data);
    let mut out = Vec::with_capacity(trimmed.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in trimmed {
        acc = (acc << 6) | value(*c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pasted_control_sequences_are_removed() {
        assert_eq!(paste_text("ls\x1b[201~curl x|sh\n"), "ls[201~curl x|sh\n");
        assert_eq!(paste_text("a\tb\r\nc\u{9b}d\x07"), "a\tb\r\ncd");
    }

    #[test]
    fn base64_and_osc7_urls() {
        assert_eq!(decode_base64(b"aGVsbG8=").unwrap(), b"hello");
        assert_eq!(decode_base64(b"aGk=").unwrap(), b"hi");
        assert_eq!(decode_base64(b"YWJj").unwrap(), b"abc");
        assert!(decode_base64(b"a$b").is_none());
        let dir = tempfile::tempdir().unwrap();
        let url = format!("file://localhost{}", dir.path().display());
        assert_eq!(cwd_from_url(&url).unwrap(), dir.path());
        assert!(cwd_from_url("file://elsewhere.example/tmp").is_none());
    }

    #[test]
    fn queries_are_answered_in_order_with_the_screen_at_that_point() {
        let mut parser = vt100::Parser::new_with_callbacks(5, 20, 0, Hooks::default());
        // The cursor moves between two position reports in one chunk.
        parser.process(b"ab\x1b[6n\x1b[3;4H\x1b[6n\x1b[c\x1b[>c\x1b]2;build\x07\x1b]52;c;aGk=\x07");
        let hooks = parser.callbacks();
        assert_eq!(
            String::from_utf8_lossy(&hooks.replies),
            "\x1b[1;3R\x1b[3;4R\x1b[?1;2c\x1b[>0;10;1c"
        );
        assert_eq!(hooks.title.as_deref(), Some("build"));
        assert_eq!(hooks.clipboard, ["hi"]);
    }
}
