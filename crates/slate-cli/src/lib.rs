use anyhow::Result;
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
        MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    prelude::*,
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};
use slate_core::{layout::Axis, terminal::Screen, App, Command, Key, Snapshot};
use std::{
    io::{self, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

mod colors;
mod ide;
use colors::ColorMode;

/// Set by SIGHUP/SIGTERM; the main loop saves unsaved work and exits.
static TERMINATE: AtomicBool = AtomicBool::new(false);
#[cfg(unix)]
extern "C" fn on_terminate(_: libc::c_int) {
    TERMINATE.store(true, Ordering::SeqCst);
}

struct InputReader {
    running: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    idle: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl InputReader {
    /// Stop reading the terminal so another program (sudo, the shell after
    /// suspend) receives the keyboard.
    fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
        let deadline = std::time::Instant::now() + Duration::from_millis(500);
        while !self.idle.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }
}
impl Drop for InputReader {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        self.paused.store(false, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Terminal modes Slate enables; restored on exit, suspend and sudo handover.
#[derive(Clone, Copy)]
struct Modes {
    mouse: bool,
    enhanced: bool,
}
fn enter(modes: Modes) -> io::Result<()> {
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)?;
    if modes.mouse {
        execute!(io::stdout(), EnableMouseCapture)?;
    }
    if modes.enhanced {
        execute!(
            io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
    }
    Ok(())
}
fn leave(modes: Modes) {
    let mut out = io::stdout();
    if modes.enhanced {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(
        out,
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen,
        crossterm::cursor::Show
    );
    let _ = disable_raw_mode();
}

struct Guard(Arc<std::sync::Mutex<Modes>>);
impl Drop for Guard {
    fn drop(&mut self) {
        leave(*self.0.lock().unwrap());
    }
}
// Rendering and mouse hit testing share terminal-column geometry, including
// wide Unicode filenames and the space reserved for each close button.
struct TabRegion {
    index: usize,
    x: u16,
    width: u16,
    close: Option<u16>,
}
fn tab_regions(tabs: &[slate_core::Tab], width: u16) -> Vec<TabRegion> {
    let widths: Vec<usize> = tabs
        .iter()
        .map(|tab| {
            Span::raw(clean(&tab.title)).width() + 2 + if tab.close_id.is_some() { 3 } else { 0 }
        })
        .collect();
    let crowded = widths.iter().sum::<usize>() > width as usize;
    let mut x = 0u16;
    let mut regions = Vec::new();
    for (index, tab) in tabs.iter().enumerate() {
        if crowded && !tab.active {
            continue;
        }
        let size = widths[index].min(width.saturating_sub(x) as usize) as u16;
        if size == 0 {
            break;
        }
        regions.push(TabRegion {
            index,
            x,
            width: size,
            close: (tab.close_id.is_some() && size >= 3).then(|| x + size - 3),
        });
        x += size;
    }
    regions
}

/// Prompts answered with a single key (no text field).
const CHOICE_PROMPTS: &[&str] = &[
    "close-tab",
    "quit",
    "save-read-only",
    "save-elevated",
    "reload-changed",
    "file-changed",
    "trust",
    "confirm-replace",
];

/// Whether the tty's erase character is ^H, making Ctrl+H Backspace.
fn erase_is_control_h() -> bool {
    #[cfg(unix)]
    {
        let fd = std::fs::File::open("/dev/tty");
        let Ok(fd) = fd else { return false };
        use std::os::unix::io::AsRawFd;
        let mut termios: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(fd.as_raw_fd(), &mut termios) } == 0 {
            return termios.c_cc[libc::VERASE] == 0x08;
        }
    }
    false
}

/// Programs that hold the desktop clipboard, when a display is available.
fn clipboard_programs(read: bool) -> Vec<(&'static str, &'static [&'static str])> {
    let mut programs: Vec<(&'static str, &'static [&'static str])> = vec![];
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        programs.push(if read {
            ("wl-paste", &["--no-newline"])
        } else {
            ("wl-copy", &[])
        });
    }
    if std::env::var_os("DISPLAY").is_some() {
        programs.push(if read {
            ("xclip", &["-o", "-selection", "clipboard"])
        } else {
            ("xclip", &["-selection", "clipboard"])
        });
        programs.push(if read {
            ("xsel", &["-ob"])
        } else {
            ("xsel", &["-ib"])
        });
    }
    programs
        .into_iter()
        .filter(|(p, _)| slate_core::fsio::which(p).is_some())
        .collect()
}
fn write_system_clipboard(text: &str) {
    if let Some((program, args)) = clipboard_programs(false).into_iter().next() {
        let text = text.to_string();
        std::thread::spawn(move || {
            if let Ok(mut child) = std::process::Command::new(program)
                .args(args)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text.as_bytes());
                }
                let _ = child.wait();
            }
        });
    }
}
fn read_system_clipboard() -> Option<String> {
    let (program, args) = clipboard_programs(true).into_iter().next()?;
    let output = std::process::Command::new(program)
        .args(args)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn run(mut app: App) -> Result<()> {
    #[cfg(unix)]
    unsafe {
        libc::signal(
            libc::SIGHUP,
            on_terminate as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGTERM,
            on_terminate as *const () as libc::sighandler_t,
        );
    }
    if slate_core::fsio::which("sudo").is_some() && unsafe { libc::geteuid() } != 0 {
        app.elevation_mode = slate_core::ElevationMode::Terminal;
    }
    let colors = ColorMode::detect();
    let modes = Arc::new(std::sync::Mutex::new(Modes {
        mouse: app.preferences.tui_mouse,
        enhanced: false,
    }));
    // Request the kitty keyboard protocol without querying for it: a query
    // stalls startup on terminals that never answer, while terminals without
    // the protocol ignore the request. Legacy key decoding stays correct in
    // both cases, because the remapped Ctrl+digit chords are otherwise unused.
    let term = std::env::var("TERM").unwrap_or_default();
    let enhanced = std::env::var_os("SLATE_NO_KEYBOARD_PROTOCOL").is_none()
        && !matches!(term.as_str(), "linux" | "dumb")
        && !term.starts_with("vt");
    modes.lock().unwrap().enhanced = enhanced;
    let _guard = Guard(modes.clone());
    enter(*modes.lock().unwrap())?;
    // Restore the outer terminal before a panic message is printed.
    let hook_modes = modes.clone();
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        leave(*hook_modes.lock().unwrap_or_else(|e| e.into_inner()));
        default_hook(info);
    }));
    let backspace_is_ctrl_h = erase_is_control_h();
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let events = app.events();
    let (input_tx, input_rx) = std::sync::mpsc::channel();
    let running = Arc::new(AtomicBool::new(true));
    let paused = Arc::new(AtomicBool::new(false));
    let idle = Arc::new(AtomicBool::new(false));
    let (reader_running, reader_paused, reader_idle) =
        (running.clone(), paused.clone(), idle.clone());
    let input_events = events.clone();
    let thread = std::thread::spawn(move || {
        while reader_running.load(Ordering::Acquire) {
            if reader_paused.load(Ordering::SeqCst) {
                reader_idle.store(true, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            reader_idle.store(false, Ordering::SeqCst);
            match event::poll(Duration::from_millis(50)) {
                Ok(false) => continue,
                Ok(true) => {
                    if reader_paused.load(Ordering::SeqCst) {
                        continue;
                    }
                    let result = event::read().map_err(|e| e.to_string());
                    let failed = result.is_err();
                    if input_tx.send(result).is_err() {
                        break;
                    }
                    input_events.notify();
                    if failed {
                        break;
                    }
                }
                Err(e) => {
                    let _ = input_tx.send(Err(e.to_string()));
                    input_events.notify();
                    break;
                }
            }
        }
    });
    let reader = InputReader {
        running,
        paused,
        idle,
        thread: Some(thread),
    };
    // Hand the terminal to another program, then take it back.
    let handover = |terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
                    work: &mut dyn FnMut()|
     -> Result<()> {
        reader.pause();
        let current = *modes.lock().unwrap();
        leave(current);
        work();
        enter(current)?;
        // A fresh Terminal forgets the previous frame, so the next draw paints
        // everything. Terminal::clear would query the cursor position, whose
        // reply the input thread could consume.
        execute!(
            io::stdout(),
            crossterm::terminal::Clear(crossterm::terminal::ClearType::All)
        )?;
        *terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        reader.resume();
        Ok(())
    };
    let mut palette: Option<String> = None;
    let mut palette_index = 0usize;
    let mut redraw = true;
    let mut prior_revision = 0;
    let mut prior_size = terminal.size()?;
    let mut snapshot = app.snapshot(
        prior_size.width,
        prior_size.height.saturating_sub(2),
        1,
        1,
        1,
        3,
    );
    let mut drag = None;
    let mut selecting = None;
    let mut terminal_capture = None;
    let mut clipboard = String::new();
    while !app.quit {
        if TERMINATE.load(Ordering::SeqCst) {
            // The terminal is going away. Keep unsaved work: recovery
            // checkpoints for workspaces, nano-style NAME.save files otherwise.
            if !app.wants_recovery() {
                let saved = app.emergency_save();
                if !saved.is_empty() {
                    eprintln!(
                        "Slate saved unsaved buffers to: {}",
                        saved
                            .iter()
                            .map(|p| p.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
            }
            break;
        }
        let observed = events.generation();
        app.process_events();
        if let Some(request) = app.take_elevation() {
            let mut result = Ok(());
            handover(&mut terminal, &mut || {
                println!(
                    "Slate: saving {} with sudo (Ctrl+C cancels)",
                    request.path.display()
                );
                result =
                    slate_core::fsio::write_elevated(&request.path, &request.bytes, "sudo", true);
            })?;
            app.finish_elevation(request, result);
            redraw = true;
        }
        if app.suspend_requested {
            app.suspend_requested = false;
            handover(&mut terminal, &mut || {
                println!("Slate suspended; type fg to resume");
                // Like nano: SIGSTOP the process group. SIGTSTP is discarded
                // when the group is orphaned (no job-control shell above it).
                #[cfg(unix)]
                unsafe {
                    libc::kill(0, libc::SIGSTOP);
                }
            })?;
            prior_revision = 0;
        }
        let wanted_mouse = app.preferences.tui_mouse;
        if modes.lock().unwrap().mouse != wanted_mouse {
            modes.lock().unwrap().mouse = wanted_mouse;
            if wanted_mouse {
                execute!(io::stdout(), EnableMouseCapture)?;
            } else {
                execute!(io::stdout(), DisableMouseCapture)?;
            }
        }
        let size = terminal.size()?;
        if app.revision() != prior_revision || size != prior_size {
            let revision = app.revision();
            snapshot = app.snapshot(size.width, size.height.saturating_sub(2), 1, 1, 1, 3);
            prior_revision = revision;
            prior_size = size;
            redraw = true;
        }
        let mut catalog = app.command_catalog(palette.as_deref().unwrap_or_default());
        if redraw {
            terminal.draw(|frame| {
                render(
                    frame,
                    &snapshot,
                    palette.as_deref(),
                    palette_index,
                    &catalog,
                    colors,
                )
            })?;
            redraw = false;
        }
        if app.clipboard != clipboard {
            clipboard = app.clipboard.clone();
            if !clipboard.is_empty() {
                use base64::Engine;
                let encoded =
                    base64::engine::general_purpose::STANDARD.encode(clipboard.as_bytes());
                write!(io::stdout(), "\x1b]52;c;{encoded}\x07")?;
                io::stdout().flush()?;
                write_system_clipboard(&clipboard);
            }
        }
        let first = match input_rx.try_recv() {
            Ok(event) => event.map_err(|e| anyhow::anyhow!(e))?,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                events.wait(observed, Duration::from_secs(1));
                continue;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                anyhow::bail!("Terminal input disconnected")
            }
        };
        redraw = true;
        // Handle typed-ahead input and paste bursts before drawing again. Hit
        // testing and the palette still see the state after each event.
        let mut next = Some(first);
        let mut burst = 0;
        while let Some(input) = next.take() {
            burst += 1;
            if burst > 1 {
                if app.revision() != prior_revision {
                    prior_revision = app.revision();
                    snapshot = app.snapshot(
                        prior_size.width,
                        prior_size.height.saturating_sub(2),
                        1,
                        1,
                        1,
                        3,
                    );
                }
                catalog = app.command_catalog(palette.as_deref().unwrap_or_default());
            }
            if burst < 256 {
                if let Ok(event) = input_rx.try_recv() {
                    next = Some(event.map_err(|e| anyhow::anyhow!(e))?);
                }
            }
            match input {
                Event::Key(k) if k.kind != KeyEventKind::Release => {
                    let k = normalize_key(k, false, backspace_is_ctrl_h);
                    if app
                        .prompt
                        .as_ref()
                        .is_some_and(|p| CHOICE_PROMPTS.contains(&p.kind.as_str()))
                    {
                        app.dispatch(Command::Key { key: translate(k) });
                        continue;
                    }
                    if let Some(line) = palette.as_mut() {
                        match k.code {
                            KeyCode::Esc => palette = None,
                            KeyCode::Enter => {
                                let query = line.clone();
                                if let Some(command) = query.strip_prefix(':') {
                                    palette = None;
                                    app.command_line(command);
                                } else if let Some(action) =
                                    catalog.get(palette_index).filter(|c| c.enabled)
                                {
                                    let id = action.id.clone();
                                    palette = None;
                                    if id == "paste" {
                                        refresh_clipboard(&mut app);
                                    }
                                    app.dispatch(Command::InvokeAction {
                                        id,
                                        argument: String::new(),
                                    });
                                }
                            }
                            KeyCode::Up => palette_index = palette_index.saturating_sub(1),
                            KeyCode::Down => {
                                palette_index =
                                    (palette_index + 1).min(catalog.len().saturating_sub(1))
                            }
                            KeyCode::Backspace => {
                                line.pop();
                                palette_index = 0;
                            }
                            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                                line.push(c);
                                palette_index = 0;
                            }
                            _ => {}
                        }
                    } else if k.code == KeyCode::F(1)
                        || (k.code == KeyCode::Char('P') || k.code == KeyCode::Char('p'))
                            && k.modifiers
                                .contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT)
                    {
                        palette = Some(String::new());
                        palette_index = 0;
                    } else {
                        let key = translate(k);
                        if app.key_binding(&key) == Some("paste") {
                            refresh_clipboard(&mut app);
                        }
                        app.dispatch(Command::Key { key });
                    }
                }
                Event::Paste(text) => {
                    if app
                        .prompt
                        .as_ref()
                        .is_some_and(|p| CHOICE_PROMPTS.contains(&p.kind.as_str()))
                    {
                        continue;
                    }
                    if let Some(line) = palette.as_mut() {
                        line.push_str(&text.replace(['\n', '\r'], " "));
                    } else {
                        app.dispatch(Command::Paste { text });
                    }
                }
                Event::Mouse(m) => {
                    if app.prompt.is_some() || palette.is_some() || snapshot.picker.is_some() {
                        continue;
                    }
                    let x = m.column;
                    let y = m.row;
                    let target = snapshot.panes.iter().find(|p| {
                        p.kind == "terminal"
                            && (terminal_capture == Some(p.id)
                                || inside(x, y, p.rect) && y > p.rect.y + 1)
                    });
                    if drag.is_none() {
                        if let Some(p) = target {
                            let (kind, button) = match m.kind {
                                MouseEventKind::Down(b) => ("press", b),
                                MouseEventKind::Up(b) => ("release", b),
                                MouseEventKind::Drag(b) => ("drag", b),
                                MouseEventKind::Moved => ("move", event::MouseButton::Left),
                                MouseEventKind::ScrollUp => ("wheel_up", event::MouseButton::Left),
                                MouseEventKind::ScrollDown => {
                                    ("wheel_down", event::MouseButton::Left)
                                }
                                _ => continue,
                            };
                            if kind == "press" {
                                terminal_capture = Some(p.id);
                            } else if kind == "release" {
                                terminal_capture = None;
                            }
                            let button = if kind == "move" {
                                3
                            } else {
                                match button {
                                    event::MouseButton::Left => 0,
                                    event::MouseButton::Middle => 1,
                                    event::MouseButton::Right => 2,
                                }
                            };
                            app.dispatch(Command::Pointer {
                                pane: p.id,
                                row: y.saturating_sub(p.rect.y + 2) as usize,
                                col: x.saturating_sub(p.rect.x + 1) as usize,
                                kind: kind.into(),
                                button,
                                shift: m.modifiers.contains(KeyModifiers::SHIFT),
                                ctrl: m.modifiers.contains(KeyModifiers::CONTROL),
                                alt: m.modifiers.contains(KeyModifiers::ALT),
                            });
                            continue;
                        }
                    }
                    if matches!(m.kind, MouseEventKind::Down(_)) {
                        drag = snapshot
                            .handles
                            .iter()
                            .find(|h| inside(x, y, h.rect))
                            .cloned();
                        if drag.is_none() {
                            if let Some(pane) = snapshot.panes.iter().find(|p| inside(x, y, p.rect))
                            {
                                if y == pane.rect.y + 1 {
                                    let column = x.saturating_sub(pane.rect.x + 1);
                                    for region in
                                        tab_regions(&pane.tabs, pane.rect.width.saturating_sub(2))
                                    {
                                        if column >= region.x && column < region.x + region.width {
                                            if region.close.is_some_and(|close| column >= close)
                                                && m.kind
                                                    == MouseEventKind::Down(
                                                        event::MouseButton::Left,
                                                    )
                                            {
                                                if let Some(view) = pane.tabs[region.index].close_id
                                                {
                                                    app.dispatch(Command::CloseTab {
                                                        pane: pane.id,
                                                        view,
                                                        force: false,
                                                    });
                                                }
                                            } else {
                                                app.dispatch(Command::SwitchTab {
                                                    pane: pane.id,
                                                    index: region.index,
                                                });
                                            }
                                            break;
                                        }
                                    }
                                } else if y > pane.rect.y + 1 {
                                    app.dispatch(Command::Focus { pane: pane.id });
                                    if pane.kind == "editor" {
                                        selecting = Some(pane.id);
                                        app.dispatch(Command::Click {
                                            pane: pane.id,
                                            row: y.saturating_sub(pane.rect.y + 2) as usize,
                                            col: x.saturating_sub(pane.rect.x + 1) as usize,
                                            shift: m.modifiers.contains(KeyModifiers::SHIFT),
                                        });
                                    } else if pane.kind == "files" || pane.kind == "git" {
                                        let len = if pane.kind == "files" {
                                            snapshot.files.len()
                                        } else {
                                            snapshot.git.len()
                                        };
                                        let height = pane.rect.height.saturating_sub(3) as usize;
                                        let top = pane
                                            .selected
                                            .saturating_sub(height.saturating_sub(1))
                                            .min(len.saturating_sub(height));
                                        let row = top + y.saturating_sub(pane.rect.y + 2) as usize;
                                        app.dispatch(Command::Click {
                                            pane: pane.id,
                                            row,
                                            col: 0,
                                            shift: false,
                                        });
                                        app.dispatch(Command::Key {
                                            key: Key {
                                                key: "Enter".into(),
                                                ..Default::default()
                                            },
                                        });
                                    }
                                }
                            }
                        }
                    } else if matches!(m.kind, MouseEventKind::Drag(_)) {
                        if let Some(h) = drag.as_ref() {
                            let ratio = if h.axis == Axis::Horizontal {
                                (x.saturating_sub(h.parent.x)) as f32 / h.parent.width.max(1) as f32
                            } else {
                                (y.saturating_sub(h.parent.y)) as f32
                                    / h.parent.height.max(1) as f32
                            };
                            app.dispatch(Command::ResizeSplit { id: h.id, ratio });
                        } else if let Some(id) = selecting {
                            if let Some(p) = snapshot.panes.iter().find(|p| p.id == id) {
                                app.dispatch(Command::Click {
                                    pane: id,
                                    row: y.saturating_sub(p.rect.y + 2) as usize,
                                    col: x.saturating_sub(p.rect.x + 1) as usize,
                                    shift: true,
                                });
                            }
                        }
                    } else if matches!(m.kind, MouseEventKind::Up(_)) {
                        drag = None;
                        selecting = None;
                    } else if matches!(
                        m.kind,
                        MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                    ) {
                        if let Some(p) = snapshot.panes.iter().find(|p| inside(x, y, p.rect)) {
                            let delta = if m.kind == MouseEventKind::ScrollUp {
                                3
                            } else {
                                -3
                            };
                            app.dispatch(Command::Scroll { pane: p.id, delta });
                        }
                    }
                }
                _ => {}
            }
        }
    }
    terminal.show_cursor()?;
    let _ = std::panic::take_hook();
    Ok(())
}
/// Read the desktop clipboard into Slate's clipboard before pasting.
fn refresh_clipboard(app: &mut App) {
    if app.focused_kind() != "editor" {
        return;
    }
    if let Some(text) = read_system_clipboard().filter(|t| !t.is_empty()) {
        if text != app.clipboard {
            app.dispatch(Command::SetClipboard { text });
        }
    }
}
fn inside(x: u16, y: u16, r: slate_core::layout::Rect) -> bool {
    x >= r.x && y >= r.y && x < r.x + r.width && y < r.y + r.height
}
/// Undo the legacy control-byte encodings crossterm reports without the
/// keyboard protocol: 0x1C–0x1F arrive as Ctrl+4..7 and ^H may be Backspace.
pub fn normalize_key(mut k: KeyEvent, enhanced: bool, backspace_is_ctrl_h: bool) -> KeyEvent {
    if enhanced || !k.modifiers.contains(KeyModifiers::CONTROL) {
        return k;
    }
    if let KeyCode::Char(c) = k.code {
        k.code = match c {
            '4' => KeyCode::Char('\\'),
            '5' => KeyCode::Char(']'),
            '7' => KeyCode::Char('_'),
            'h' if backspace_is_ctrl_h && k.modifiers == KeyModifiers::CONTROL => {
                k.modifiers = KeyModifiers::NONE;
                KeyCode::Backspace
            }
            other => KeyCode::Char(other),
        };
    }
    k
}
pub fn translate(k: KeyEvent) -> Key {
    let (key, text) = match k.code {
        KeyCode::Char(c) => (c.to_string(), c.to_string()),
        KeyCode::Enter => ("Enter".into(), String::new()),
        KeyCode::Backspace => ("Backspace".into(), String::new()),
        KeyCode::Delete => ("Delete".into(), String::new()),
        KeyCode::Insert => ("Insert".into(), String::new()),
        KeyCode::Tab => ("Tab".into(), String::new()),
        KeyCode::BackTab => ("Tab".into(), String::new()),
        KeyCode::Esc => ("Escape".into(), String::new()),
        KeyCode::Up => ("Up".into(), String::new()),
        KeyCode::Down => ("Down".into(), String::new()),
        KeyCode::Left => ("Left".into(), String::new()),
        KeyCode::Right => ("Right".into(), String::new()),
        KeyCode::Home => ("Home".into(), String::new()),
        KeyCode::End => ("End".into(), String::new()),
        KeyCode::PageUp => ("PageUp".into(), String::new()),
        KeyCode::PageDown => ("PageDown".into(), String::new()),
        KeyCode::F(n) => (format!("F{n}"), String::new()),
        _ => (String::new(), String::new()),
    };
    Key {
        key,
        text,
        ctrl: k.modifiers.contains(KeyModifiers::CONTROL),
        alt: k.modifiers.contains(KeyModifiers::ALT),
        shift: k.modifiers.contains(KeyModifiers::SHIFT) || k.code == KeyCode::BackTab,
    }
}
fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { '�' } else { c })
        .collect()
}
/// Text, title and height of each prompt kind.
fn prompt_view(prompt: &slate_core::search::Prompt) -> (String, &'static str, u16) {
    let input = clean(&prompt.input);
    match prompt.kind.as_str() {
        "close-tab" => (
            format!("Discard unsaved changes in {input}?\nD: discard and close · Escape / Enter: cancel\nCancel to save the document first."),
            "Unsaved changes",
            6,
        ),
        "quit" => (
            format!("Save changes before quitting? ({input})\nY: save all and quit · N: discard and quit · C / Escape: cancel"),
            "Unsaved changes",
            5,
        ),
        "save-read-only" => (
            format!("{input} is read-only.\nY: overwrite it anyway · N / Escape: cancel"),
            "Read-only file",
            5,
        ),
        "save-elevated" => (
            format!("You do not have permission to write {input}.\nY: save with sudo (asks for your password) · N / Escape: cancel"),
            "Permission denied",
            5,
        ),
        "file-changed" => (
            format!("{input} changed on disk while you have unsaved changes.\nR: reload it (discard your edits) · K: keep your version (saving replaces the file) · Escape: decide later"),
            "File changed on disk",
            5,
        ),
        "confirm-replace" => (
            format!("Replace every search result in {input}?\nY: replace (open documents stay unsaved) · N / Escape: cancel"),
            "Replace in files",
            5,
        ),
        "trust" => (
            format!("Trust {input}?\nY: trust this folder (its tools may run) · N / Escape: stay in restricted mode"),
            "Workspace trust",
            5,
        ),
        "reload-changed" => (
            format!("Reload {input} from disk and discard your unsaved changes?\nY: reload · N / Escape: keep editing"),
            "Reload",
            5,
        ),
        "replace" => (
            format!("Find: {input}\nWith: {}\nTab switches fields · Enter replaces next · Ctrl-Enter replaces all\nEscape closes", clean(&prompt.replacement)),
            "replace",
            8,
        ),
        kind => (
            format!("> {input}\nEnter confirms · Escape closes"),
            match kind {
                "open" => "Open file or directory",
                "save-as" => "Save to a new file path",
                "commit" => "Commit message",
                "layout-save" => "Save layout name",
                "layout-load" => "Load layout name",
                "move-pane" => "Target pane number",
                "insert-file" => "Insert file",
                "set-encoding" => "Encoding for saving (utf-8, utf-16le, windows-1252…)",
                "reopen-encoding" => "Reopen with encoding (utf-8, latin1, shift_jis…)",
                "set-line-ending" => "Line endings: lf, crlf or cr",
                "open-recent" => "Open recent file (path or number)",
                "rename-symbol" => "Rename symbol to",
                "project-replace" => "Replace search results with",
                "debug-evaluate" => "Evaluate expression",
                "debug-program" => "Program to debug",
                "goto" => "Go to line",
                "find" => "Find",
                _ => "Input",
            },
            6,
        ),
    }
}
fn render(
    frame: &mut Frame,
    s: &Snapshot,
    palette: Option<&str>,
    palette_index: usize,
    catalog: &[slate_core::commands::CommandInfo],
    colors: ColorMode,
) {
    let base = colors.style(&s.foreground, &s.background);
    for p in &s.panes {
        let area = Rect::new(p.rect.x, p.rect.y, p.rect.width, p.rect.height);
        let focus = p.id == s.focus;
        let block = Block::default()
            .borders(Borders::ALL)
            .style(base)
            .title(if p.kind == "git" {
                format!(
                    " Git: {} #{} ",
                    if s.git_repository {
                        &s.git_branch
                    } else {
                        "No repository"
                    },
                    p.id
                )
            } else {
                format!(" {} #{} ", p.kind, p.id)
            })
            .border_style(colors.border(focus));
        frame.render_widget(block, area);
        for region in tab_regions(&p.tabs, area.width.saturating_sub(2)) {
            let tab = &p.tabs[region.index];
            let label_width =
                region
                    .width
                    .saturating_sub(if region.close.is_some() { 3 } else { 0 });
            frame.render_widget(
                Paragraph::new(format!(" {} ", clean(&tab.title))).style(colors.tab(tab.active)),
                Rect::new(area.x + 1 + region.x, area.y + 1, label_width, 1),
            );
            if let Some(close) = region.close {
                frame.render_widget(
                    Paragraph::new("[x]").style(colors.tab(false)),
                    Rect::new(area.x + 1 + close, area.y + 1, 3, 1),
                );
            }
        }
        let content = Rect::new(
            area.x + 1,
            area.y + 2,
            area.width.saturating_sub(2),
            area.height.saturating_sub(3),
        );
        if let Some(screen) = &p.screen {
            frame.render_widget(
                Grid {
                    screen,
                    colors,
                    selection: &s.selection,
                },
                content,
            );
            if focus && palette.is_none() && s.prompt.is_none() && s.picker.is_none() {
                if let Some((y, x)) = screen.cursor {
                    if x < content.width && y < content.height {
                        frame.set_cursor_position((content.x + x, content.y + y));
                    }
                }
            }
        } else {
            let items: Vec<ListItem> = if p.kind == "files" {
                s.files
                    .iter()
                    .map(|e| {
                        ListItem::new(clean(&format!(
                            "{} {}",
                            if e.directory { "▸" } else { " " },
                            e.name
                        )))
                    })
                    .collect()
            } else {
                s.git
                    .iter()
                    .map(|e| {
                        ListItem::new(clean(&format!("[{}] {} {}", e.group, e.status, e.path)))
                    })
                    .collect()
            };
            let mut state = ListState::default().with_selected(Some(p.selected));
            frame.render_stateful_widget(
                List::new(items).highlight_style(colors.highlight()),
                content,
                &mut state,
            );
        }
    }
    let size = frame.area();
    frame.render_widget(
        Paragraph::new(clean(&format!(
            "{} · {}{}",
            s.status,
            s.location,
            ide::status_suffix(s)
        )))
        .style(colors.status()),
        Rect::new(0, size.height.saturating_sub(2), size.width, 1),
    );
    frame.render_widget(
        Paragraph::new(s.hints.as_str()),
        Rect::new(0, size.height.saturating_sub(1), size.width, 1),
    );
    ide::render_hover(frame, s);
    ide::render_completion(frame, s, colors);
    if s.prompt.is_none() && palette.is_none() {
        ide::render_picker(frame, s, colors);
    }
    if let Some(prompt) = &s.prompt {
        let settings = prompt.kind == "settings";
        let (text, title, height) = prompt_view(prompt);
        let height = if settings {
            (s.settings.menu_items().len() as u16 + 4).min(size.height.saturating_sub(2))
        } else {
            height
        };
        let area = Rect::new(
            1,
            size.height.saturating_sub(height + 1),
            size.width.saturating_sub(2),
            height,
        );
        frame.render_widget(Clear, area);
        if settings {
            let items = s
                .settings
                .menu_items()
                .into_iter()
                .map(|(label, value)| ListItem::new(format!("{label}: {value}")))
                .collect::<Vec<_>>();
            frame.render_widget(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Settings · saved automatically"),
                area,
            );
            let mut state = ListState::default().with_selected(Some(prompt.field));
            frame.render_stateful_widget(
                List::new(items).highlight_style(colors.highlight()),
                Rect::new(
                    area.x + 1,
                    area.y + 1,
                    area.width.saturating_sub(2),
                    area.height.saturating_sub(4),
                ),
                &mut state,
            );
            frame.render_widget(
                Paragraph::new("↑↓ select · ←→ / Enter change · Escape closes"),
                Rect::new(
                    area.x + 1,
                    area.y + area.height.saturating_sub(2),
                    area.width.saturating_sub(2),
                    1,
                ),
            );
        } else {
            let text = if ["find", "replace"].contains(&prompt.kind.as_str()) {
                format!(
                    "{text}\nAlt-C case: {} · Alt-W whole word: {}",
                    if prompt.case_sensitive { "on" } else { "off" },
                    if prompt.whole_word { "on" } else { "off" }
                )
            } else {
                text
            };
            frame.render_widget(
                Paragraph::new(text).block(Block::default().borders(Borders::ALL).title(title)),
                area,
            );
            if !CHOICE_PROMPTS.contains(&prompt.kind.as_str()) {
                let value = if prompt.field == 0 {
                    &prompt.input
                } else {
                    &prompt.replacement
                };
                let prefix = if prompt.kind == "replace" { 6 } else { 3 };
                frame.set_cursor_position((
                    area.x
                        + (prefix + slate_core::document::display_width(value) as u16)
                            .min(area.width.saturating_sub(2)),
                    area.y + 1 + prompt.field as u16,
                ));
            }
        }
    }
    if let Some(text) = palette {
        let height = size.height.saturating_sub(4).min(16);
        let area = Rect::new(
            1,
            size.height.saturating_sub(height + 1),
            size.width.saturating_sub(2),
            height,
        );
        frame.render_widget(Clear, area);
        frame.render_widget(
            Block::default()
                .borders(Borders::ALL)
                .title("Commands · ↑↓ select · Enter runs · Escape closes"),
            area,
        );
        frame.render_widget(
            Paragraph::new(format!("> {}", clean(text))),
            Rect::new(area.x + 1, area.y + 1, area.width.saturating_sub(2), 1),
        );
        let items = catalog
            .iter()
            .map(|c| {
                ListItem::new(format!(
                    "{}  {}\n  {}",
                    c.name,
                    c.shortcut,
                    if c.enabled { &c.description } else { &c.reason }
                ))
                .style(colors.palette_item(c.enabled))
            })
            .collect::<Vec<_>>();
        let mut state = ListState::default().with_selected(Some(palette_index));
        frame.render_stateful_widget(
            List::new(items).highlight_style(colors.highlight()),
            Rect::new(
                area.x + 1,
                area.y + 2,
                area.width.saturating_sub(2),
                area.height.saturating_sub(3),
            ),
            &mut state,
        );
        if catalog.is_empty() {
            frame.render_widget(
                Paragraph::new(if text.starts_with(':') {
                    "Enter executes the typed command"
                } else {
                    "No matching actions"
                }),
                Rect::new(area.x + 1, area.y + 2, area.width.saturating_sub(2), 1),
            );
        }
        frame.set_cursor_position((
            area.x
                + 3
                + (slate_core::document::display_width(text) as u16)
                    .min(area.width.saturating_sub(5)),
            area.y + 1,
        ));
    }
}
struct Grid<'a> {
    screen: &'a Screen,
    colors: ColorMode,
    selection: &'a str,
}
impl Widget for Grid<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        for (row, line) in self
            .screen
            .cells
            .iter()
            .take(area.height as usize)
            .enumerate()
        {
            for (col, c) in line.iter().take(area.width as usize).enumerate() {
                let x = area.x + col as u16;
                let y = area.y + row as u16;
                let cell = &mut buf[(x, y)];
                let mut modifiers = Modifier::empty();
                if c.bold {
                    modifiers |= Modifier::BOLD;
                }
                if c.italic {
                    modifiers |= Modifier::ITALIC;
                }
                if c.underline {
                    modifiers |= Modifier::UNDERLINED;
                }
                let mut style = self.colors.style(&c.fg, &c.bg).add_modifier(modifiers);
                if self.colors == ColorMode::None && c.bg == self.selection {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                cell.set_style(style);
                if !c.continuation {
                    cell.set_symbol(if c.text.is_empty() { " " } else { &c.text });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_control_bytes_map_to_their_punctuation() {
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        assert_eq!(
            normalize_key(ctrl('5'), false, false).code,
            KeyCode::Char(']')
        );
        assert_eq!(
            normalize_key(ctrl('4'), false, false).code,
            KeyCode::Char('\\')
        );
        assert_eq!(
            normalize_key(ctrl('7'), false, false).code,
            KeyCode::Char('_')
        );
        // With the keyboard protocol, digits are real digits.
        assert_eq!(
            normalize_key(ctrl('5'), true, false).code,
            KeyCode::Char('5')
        );
        let backspace = normalize_key(ctrl('h'), false, true);
        assert_eq!(backspace.code, KeyCode::Backspace);
        assert!(backspace.modifiers.is_empty());
        assert_eq!(
            normalize_key(ctrl('h'), false, false).code,
            KeyCode::Char('h')
        );
        let chord = slate_core::key_chord(&translate(normalize_key(ctrl('5'), false, false)));
        assert_eq!(chord, "Ctrl+]");
    }
}
