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
    os::fd::{AsRawFd, RawFd},
    sync::{
        atomic::{AtomicBool, AtomicI32, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

mod colors;
mod ide;
use colors::ColorMode;

/// Set by SIGHUP/SIGTERM; the main loop saves unsaved work and exits.
static TERMINATE: AtomicBool = AtomicBool::new(false);
/// The input thread sleeps until the terminal has input or this pipe is
/// written: by signal handlers, pause/resume and shutdown. Its write end is
/// never closed, so a signal handler can always use the descriptor.
static WAKE_PIPE: std::sync::OnceLock<(
    std::os::unix::net::UnixStream,
    std::os::unix::net::UnixStream,
)> = std::sync::OnceLock::new();
static WAKE_FD: AtomicI32 = AtomicI32::new(-1);
/// Async-signal-safe.
fn wake_input() {
    let fd = WAKE_FD.load(Ordering::SeqCst);
    if fd >= 0 {
        #[cfg(target_os = "linux")]
        let errno = unsafe { *libc::__errno_location() };
        unsafe {
            libc::write(fd, [1u8].as_ptr().cast(), 1);
        }
        #[cfg(target_os = "linux")]
        unsafe {
            *libc::__errno_location() = errno;
        }
    }
}
/// The read end of the wake pipe, created on first use.
fn wake_reader() -> io::Result<RawFd> {
    if WAKE_PIPE.get().is_none() {
        let (reader, writer) = std::os::unix::net::UnixStream::pair()?;
        reader.set_nonblocking(true)?;
        writer.set_nonblocking(true)?;
        let _ = WAKE_PIPE.set((reader, writer));
    }
    let (reader, writer) = WAKE_PIPE.get().unwrap();
    WAKE_FD.store(writer.as_raw_fd(), Ordering::SeqCst);
    Ok(reader.as_raw_fd())
}
fn drain_wakes(fd: RawFd) {
    let mut bytes = [0u8; 64];
    while unsafe { libc::read(fd, bytes.as_mut_ptr().cast(), bytes.len()) } > 0 {}
}
/// Block until one of `fds` is readable; reports which ones are.
fn wait_readable<const N: usize>(fds: [RawFd; N]) -> [bool; N] {
    let mut polled = fds.map(|fd| libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    });
    if unsafe { libc::poll(polled.as_mut_ptr(), N as libc::nfds_t, -1) } <= 0 {
        // Interrupted by a signal: its handler wrote the wake pipe.
        return [false; N];
    }
    polled.map(|p| p.revents != 0)
}
#[cfg(unix)]
extern "C" fn on_terminate(_: libc::c_int) {
    TERMINATE.store(true, Ordering::SeqCst);
    wake_input();
}
/// The main loop measures the terminal when it wakes.
#[cfg(unix)]
extern "C" fn on_resize(_: libc::c_int) {
    wake_input();
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
        wake_input();
        let deadline = std::time::Instant::now() + Duration::from_millis(500);
        while !self.idle.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
        wake_input();
    }
}
impl Drop for InputReader {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        self.paused.store(false, Ordering::SeqCst);
        wake_input();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
/// The terminal crossterm reads: standard input when it is a terminal,
/// otherwise /dev/tty.
fn terminal_input() -> io::Result<(RawFd, Option<std::fs::File>)> {
    if unsafe { libc::isatty(libc::STDIN_FILENO) } == 1 {
        return Ok((libc::STDIN_FILENO, None));
    }
    let tty = std::fs::File::open("/dev/tty")?;
    Ok((tty.as_raw_fd(), Some(tty)))
}

type PanicHook = Arc<dyn Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync>;
/// Reinstates the panic hook that was active before the interface started.
struct PanicHookGuard(Option<PanicHook>);
impl Drop for PanicHookGuard {
    fn drop(&mut self) {
        // Setting a hook while unwinding would abort; the process is ending.
        if let (Some(previous), false) = (self.0.take(), std::thread::panicking()) {
            let _ = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| previous(info)));
        }
    }
}
/// A panic on the interface thread restores the outer terminal before the
/// previous hook prints its message. A worker thread's panic (language
/// servers, highlighting, file I/O) leaves the screen alone: printing over
/// the running editor would garble it and leaving raw mode would break it.
/// Its message is recorded in `failures` for the interface thread to show,
/// and `notify` wakes that thread. Panics their caller catches and handles
/// (see `slate_core::panics`) are neither.
fn install_panic_hook(
    restore: impl Fn() + Send + Sync + 'static,
    failures: Arc<Mutex<Option<String>>>,
    notify: impl Fn() + Send + Sync + 'static,
) -> PanicHookGuard {
    let interface = std::thread::current().id();
    let previous: PanicHook = Arc::from(std::panic::take_hook());
    let chained = previous.clone();
    std::panic::set_hook(Box::new(move |info| {
        // Its caller catches it and reports what failed; the session is fine.
        if slate_core::panics::is_handled() {
            return;
        }
        let thread = std::thread::current();
        if thread.id() == interface {
            restore();
            chained(info);
            return;
        }
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
            .unwrap_or("unknown cause");
        let location = info
            .location()
            .map(|l| format!(" at {}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let message = format!(
            "thread '{}' panicked{location}: {}",
            thread.name().unwrap_or("<unnamed>"),
            payload.chars().take(300).collect::<String>()
        );
        failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert(message);
        notify();
    }));
    PanicHookGuard(Some(previous))
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
    "switch-workspace",
    "trash-file",
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
fn clipboard_programs(read: bool) -> Vec<(std::path::PathBuf, &'static [&'static str])> {
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
    // Resolved to absolute paths: never a helper in the current folder.
    programs
        .into_iter()
        .filter_map(|(p, args)| Some((slate_core::fsio::which(p)?, args)))
        .collect()
}
fn write_system_clipboard(text: &str) {
    if let Some((program, args)) = clipboard_programs(false).into_iter().next() {
        let text = text.to_string();
        std::thread::spawn(move || {
            if let Ok(mut child) =
                slate_core::process::sanitize(&mut std::process::Command::new(program))
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
    let output = slate_core::process::sanitize(&mut std::process::Command::new(program))
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
        // Installed before crossterm reads events: its own SIGWINCH handler
        // chains to this one.
        libc::signal(libc::SIGWINCH, on_resize as *const () as libc::sighandler_t);
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
    let events = app.events();
    let worker_failures = Arc::new(Mutex::new(None));
    let hook_modes = modes.clone();
    let hook_events = events.clone();
    let _panic_hook = install_panic_hook(
        move || leave(*hook_modes.lock().unwrap_or_else(|e| e.into_inner())),
        worker_failures.clone(),
        move || hook_events.notify(),
    );
    let backspace_is_ctrl_h = erase_is_control_h();
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let (input_tx, input_rx) = std::sync::mpsc::channel();
    let running = Arc::new(AtomicBool::new(true));
    let paused = Arc::new(AtomicBool::new(false));
    let idle = Arc::new(AtomicBool::new(false));
    let (reader_running, reader_paused, reader_idle) =
        (running.clone(), paused.clone(), idle.clone());
    let input_events = events.clone();
    let wake = wake_reader()?;
    let (tty, tty_file) = terminal_input()?;
    let thread = std::thread::spawn(move || {
        let _tty_file = tty_file;
        // Set after the terminal reported input: give crossterm a moment to
        // see it, so input it has not consumed yet cannot spin this loop.
        let mut input_ready = false;
        while reader_running.load(Ordering::Acquire) {
            if reader_paused.load(Ordering::SeqCst) {
                reader_idle.store(true, Ordering::SeqCst);
                // Another program owns the keyboard: wait for resume or exit.
                wait_readable([wake]);
                drain_wakes(wake);
                continue;
            }
            reader_idle.store(false, Ordering::SeqCst);
            let timeout = if input_ready {
                Duration::from_millis(50)
            } else {
                Duration::ZERO
            };
            match event::poll(timeout) {
                Ok(false) => {
                    // Idle: sleep until the terminal has input or a signal,
                    // pause or shutdown writes the wake pipe.
                    let [input, woken] = wait_readable([tty, wake]);
                    input_ready = input;
                    if woken {
                        drain_wakes(wake);
                        input_events.notify();
                    }
                }
                Ok(true) => {
                    input_ready = false;
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
        if let Some(failure) = worker_failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            // The interface keeps running so unsaved work can still be saved.
            app.report_internal_failure(format!(
                "A background task stopped unexpectedly ({failure}); save your documents and restart Slate"
            ));
        }
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
            // Outside raw mode Ctrl+C, Ctrl+\ and Ctrl+Z signal the
            // terminal's foreground: sudo while it runs, Slate just before
            // and after. Slate must survive them with its unsaved buffers.
            #[cfg(unix)]
            let signals = slate_core::process::HoldTerminalSignals::new();
            handover(&mut terminal, &mut || {
                println!(
                    "Slate: saving {} with sudo (Ctrl+C cancels)",
                    request.path.display()
                );
                result = slate_core::fsio::write_elevated(
                    &request.path,
                    &request.bytes,
                    request.baseline.as_ref(),
                    "sudo",
                    true,
                );
            })?;
            #[cfg(unix)]
            drop(signals);
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
        let mut catalog = palette_catalog(&app, palette.as_deref());
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
                catalog = palette_catalog(&app, palette.as_deref());
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
                                    run_palette_action(&mut app, id);
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
                    if palette.is_some() && app.prompt.is_none() {
                        let size = terminal.size()?;
                        let rows = palette_rows(Rect::new(0, 0, size.width, size.height));
                        let top = palette_top(rows, palette_index);
                        match m.kind {
                            MouseEventKind::ScrollUp => {
                                palette_index = palette_index.saturating_sub(1)
                            }
                            MouseEventKind::ScrollDown => {
                                palette_index =
                                    (palette_index + 1).min(catalog.len().saturating_sub(1))
                            }
                            MouseEventKind::Down(event::MouseButton::Left)
                                if rows.contains(Position::new(m.column, m.row)) =>
                            {
                                // Each command takes two rows.
                                let index = top + (m.row - rows.y) as usize / 2;
                                if let Some(action) = catalog.get(index).filter(|c| c.enabled) {
                                    let id = action.id.clone();
                                    palette = None;
                                    run_palette_action(&mut app, id);
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if let Some(picker) = snapshot.picker.as_ref().filter(|_| app.prompt.is_none())
                    {
                        let size = terminal.size()?;
                        let size = Rect::new(0, 0, size.width, size.height);
                        if let Some(command) =
                            ide::picker_mouse(size, picker, m.kind, m.column, m.row)
                        {
                            app.dispatch(command);
                        }
                        continue;
                    }
                    if app.prompt.is_some() || palette.is_some() {
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
fn prompt_view(
    prompt: &slate_core::search::Prompt,
    search: &slate_core::search::Search,
) -> (String, &'static str, u16) {
    let input = clean(&prompt.input);
    match prompt.kind.as_str() {
        "trash-file" => (format!("Move {input} to desktop Trash?\nY: move to Trash · N / Escape: cancel"), "Move to Trash", 5),
        "close-tab" => (
            format!("Save changes to {input} before closing?\nS: save and close · D: discard and close\nEscape / Enter: cancel"),
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
        "switch-workspace" => (format!("Open {input}?\nY / S: save all first · N / D: discard changes · Escape: cancel"), "Open folder", 5),
        "reload-changed" => (
            format!("Reload {input} from disk and discard your unsaved changes?\nY: reload · N / Escape: keep editing"),
            "Reload",
            5,
        ),
        "find" | "replace" => (
            format!("Find: {input}{}\nAlt+C case: {} · Alt+W word: {}\nAlt+R regex: {} · Alt+S selection: {}\n↑ ↓ history · {} · Escape closes",
                if prompt.kind == "replace" {format!("\nWith: {}",clean(&prompt.replacement))} else {String::new()},
                if prompt.case_sensitive {"on"} else {"off"},if prompt.whole_word {"on"} else {"off"},
                if search.regex_mode {"on"} else {"off"},if search.selection_only {"on"} else {"off"},
                if prompt.kind == "replace" {"Tab fields · Enter replaces next · Ctrl-Enter all"} else {"Enter finds"}),
            if prompt.kind == "replace" {"replace"} else {"Find"}, 9,
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
                "open-recent-project" => "Open recent project (path or number)",
                "search-settings" => "Search settings JSON (include / exclude accept semicolon-separated globs)",
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
                    selection: if p.kind == "terminal" {
                        &s.terminal_selection
                    } else {
                        &s.selection
                    },
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
            // Build only the rows that fit: lists may hold 100,000 entries.
            let total = if p.kind == "files" {
                s.files.len()
            } else {
                s.git.len()
            };
            let (rows, selected) = list_window(total, p.selected, content.height as usize);
            let items: Vec<ListItem> = if p.kind == "files" {
                s.files[rows]
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
                s.git[rows]
                    .iter()
                    .map(|e| {
                        ListItem::new(clean(&format!("[{}] {} {}", e.group, e.status, e.path)))
                    })
                    .collect()
            };
            let mut state = ListState::default().with_selected(Some(selected));
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
        let (text, title, height) = prompt_view(prompt, &s.search);
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
        let area = palette_area(size);
        frame.render_widget(Clear, area);
        frame.render_widget(
            Block::default()
                .borders(Borders::ALL)
                .title("Commands · ↑↓ select · Enter or click runs · Escape closes"),
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
        let rows = palette_rows(size);
        let mut state = ListState::default()
            .with_offset(palette_top(rows, palette_index))
            .with_selected(Some(palette_index));
        frame.render_stateful_widget(
            List::new(items).highlight_style(colors.highlight()),
            rows,
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
/// The rows of a `total`-row list that fit `height`, and the selection
/// within them: the window List scrolls to for a selection given afresh.
fn list_window(total: usize, selected: usize, height: usize) -> (std::ops::Range<usize>, usize) {
    let selected = selected.min(total.saturating_sub(1));
    let first = (selected + 1).saturating_sub(height.max(1));
    (first..total.min(first + height), selected - first)
}
/// The command palette's box on a screen of `size`.
fn palette_area(size: Rect) -> Rect {
    let height = size.height.saturating_sub(4).min(16);
    Rect::new(
        1,
        size.height.saturating_sub(height + 1),
        size.width.saturating_sub(2),
        height,
    )
}
/// The rows listing commands in the palette.
fn palette_rows(size: Rect) -> Rect {
    let area = palette_area(size);
    Rect::new(
        area.x + 1,
        area.y + 2,
        area.width.saturating_sub(2),
        area.height.saturating_sub(3),
    )
}
/// The first command drawn, keeping `selected` in view (two rows each).
fn palette_top(rows: Rect, selected: usize) -> usize {
    (selected + 1).saturating_sub((rows.height as usize / 2).max(1))
}
/// The palette's matching commands. Describing every action (with its
/// availability) is only worth doing while the palette shows them.
fn palette_catalog(app: &App, palette: Option<&str>) -> Vec<slate_core::commands::CommandInfo> {
    palette.map_or_else(Vec::new, |query| app.command_catalog(query))
}
fn run_palette_action(app: &mut App, id: String) {
    if id == "paste" {
        refresh_clipboard(app);
    }
    app.dispatch(Command::InvokeAction {
        id,
        argument: String::new(),
    });
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
    fn worker_panics_are_recorded_and_only_the_interface_restores_the_terminal() {
        use std::sync::atomic::AtomicUsize;
        let restored = Arc::new(AtomicUsize::new(0));
        let notified = Arc::new(AtomicUsize::new(0));
        let failures = Arc::new(Mutex::new(None));
        let (restore, notify) = (restored.clone(), notified.clone());
        let hook = install_panic_hook(
            move || {
                restore.fetch_add(1, Ordering::SeqCst);
            },
            failures.clone(),
            move || {
                notify.fetch_add(1, Ordering::SeqCst);
            },
        );
        let worker = std::thread::Builder::new()
            .name("highlight".into())
            .spawn(|| panic!("worker failure marker"))
            .unwrap();
        assert!(worker.join().is_err());
        // The editor's screen and raw mode stay; the main loop is told.
        assert_eq!(restored.load(Ordering::SeqCst), 0);
        assert!(notified.load(Ordering::SeqCst) >= 1);
        let failure = failures.lock().unwrap().take().unwrap();
        assert!(
            failure.starts_with("thread 'highlight' panicked at ")
                && failure.ends_with(": worker failure marker"),
            "{failure}"
        );
        // A panic its caller catches and reports is not a session failure.
        let notifications = notified.load(Ordering::SeqCst);
        let handled = std::thread::spawn(|| {
            slate_core::panics::catch_handled(|| panic!("handled marker")).is_err()
        });
        assert!(handled.join().unwrap());
        assert!(slate_core::panics::catch_handled(|| panic!("handled marker")).is_err());
        assert_eq!(notified.load(Ordering::SeqCst), notifications);
        assert_eq!(restored.load(Ordering::SeqCst), 0);
        assert!(failures.lock().unwrap().is_none());
        // The interface thread's own panic restores the terminal first.
        assert!(std::panic::catch_unwind(|| panic!("interface failure marker")).is_err());
        assert_eq!(restored.load(Ordering::SeqCst), 1);
        // Leaving the interface reinstates the previous hook.
        drop(hook);
        let notifications = notified.load(Ordering::SeqCst);
        assert!(std::thread::spawn(|| panic!("after exit marker"))
            .join()
            .is_err());
        assert_eq!(notified.load(Ordering::SeqCst), notifications);
        assert!(failures.lock().unwrap().is_none());
    }

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

    #[test]
    fn windowed_lists_draw_like_the_whole_list() {
        let draw = |items: Vec<String>, selected: usize, height: u16| {
            let area = Rect::new(0, 0, 12, height);
            let mut buffer = Buffer::empty(area);
            let mut state = ListState::default().with_selected(Some(selected));
            StatefulWidget::render(
                List::new(items.into_iter().map(ListItem::new))
                    .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
                area,
                &mut buffer,
                &mut state,
            );
            buffer
        };
        for total in [0, 1, 5, 40] {
            let items: Vec<String> = (0..total).map(|i| format!("row {i}")).collect();
            for height in [1, 3, 10] {
                for selected in 0..total + 3 {
                    let (rows, within) = list_window(total, selected, height as usize);
                    assert_eq!(
                        draw(items[rows].to_vec(), within, height),
                        draw(items.clone(), selected, height),
                        "{total} rows, {height} high, row {selected} selected"
                    );
                }
            }
        }
    }
}
