use serde::Serialize;
use slate_core::{
    text_presentation::{EditorText, TextLayout, TextSpan},
    App, Command,
};
use std::{
    collections::BTreeMap,
    ffi::{c_char, c_void, CStr, CString},
    panic::{catch_unwind, AssertUnwindSafe},
    sync::Arc,
};
extern "C" {
    fn slate_qt_run(context: *mut c_void, argc: i32, argv: *mut *mut c_char) -> i32;
}

static TERMINATED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Readable once a termination signal arrived. The GUI thread watches it, so
/// the signal thread never touches Qt (which may not exist yet).
#[cfg(unix)]
static TERMINATION_PIPE: std::sync::OnceLock<std::os::unix::net::UnixStream> =
    std::sync::OnceLock::new();

/// Turn SIGTERM, SIGHUP and SIGINT into an orderly quit: recovery state is
/// flushed (or NAME.save files written) and the single-instance socket is
/// removed. Call before starting other threads, which inherit the mask.
/// A signal that arrives before the event loop runs ends startup instead.
pub fn handle_termination_signals() {
    #[cfg(unix)]
    unsafe {
        use std::io::Write;
        let Ok((reader, mut writer)) = std::os::unix::net::UnixStream::pair() else {
            return;
        };
        if writer.set_nonblocking(true).is_err() || TERMINATION_PIPE.set(reader).is_err() {
            return;
        }
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for signal in [libc::SIGTERM, libc::SIGHUP, libc::SIGINT] {
            libc::sigaddset(&mut set, signal);
        }
        if libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) != 0 {
            return;
        }
        let waiting = set;
        std::thread::spawn(move || loop {
            let mut signal = 0;
            if libc::sigwait(&waiting, &mut signal) == 0 {
                TERMINATED.store(true, std::sync::atomic::Ordering::SeqCst);
                let _ = writer.write(&[1]);
            }
        });
    }
}
/// Whether a termination signal has arrived.
pub fn terminated() -> bool {
    TERMINATED.load(std::sync::atomic::Ordering::SeqCst)
}
#[no_mangle]
extern "C" fn slate_terminated() -> bool {
    terminated()
}
/// The descriptor Qt watches for termination signals, or -1.
#[no_mangle]
extern "C" fn slate_termination_fd() -> i32 {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        TERMINATION_PIPE.get().map_or(-1, |pipe| pipe.as_raw_fd())
    }
    #[cfg(not(unix))]
    {
        -1
    }
}
struct GuiContext {
    app: App,
    revision: u64,
    viewport: String,
    panes: BTreeMap<u64, u64>,
    editors: BTreeMap<u64, EditorText>,
    terminals: BTreeMap<u64, Arc<slate_core::terminal::Screen>>,
    settings: Option<serde_json::Value>,
    colors: Option<(String, String)>,
    files: u64,
    git: u64,
    global_keys: Option<BTreeMap<String, String>>,
}
impl GuiContext {
    fn new(app: App) -> Self {
        Self {
            app,
            revision: 0,
            viewport: String::new(),
            panes: BTreeMap::new(),
            editors: BTreeMap::new(),
            terminals: BTreeMap::new(),
            settings: None,
            colors: None,
            files: 0,
            git: 0,
            global_keys: None,
        }
    }
}
#[derive(Serialize)]
struct LinePatch {
    row: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    layout: Option<TextLayout>,
    #[serde(skip_serializing_if = "Option::is_none")]
    overlays: Option<Vec<TextSpan>>,
}
#[derive(Serialize)]
struct SurfacePatch {
    id: u64,
    kind: &'static str,
    rows: u16,
    cols: u16,
    cursor: Option<(u16, u16)>,
    line_count: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    lines: Vec<LinePatch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    screen: Option<TerminalPatch>,
}
/// A terminal row as runs of equally styled cells: `(style, columns, text)`.
/// A plain ASCII run has one character per column (blank cells are spaces);
/// any other cell is a run of its own, a wide glyph covering its
/// continuation cell.
#[derive(Serialize)]
struct TerminalRow {
    row: usize,
    runs: Vec<(usize, usize, String)>,
}
#[derive(Serialize)]
struct TerminalPatch {
    rows: u16,
    cols: u16,
    cursor: Option<(u16, u16)>,
    /// `(foreground, background, bold | italic << 1 | underline << 2)`,
    /// indexed by the runs of this patch.
    styles: Vec<(String, String, u8)>,
    lines: Vec<TerminalRow>,
}
impl TerminalPatch {
    fn new(screen: &slate_core::terminal::Screen) -> Self {
        Self {
            rows: screen.rows,
            cols: screen.cols,
            cursor: screen.cursor,
            styles: vec![],
            lines: vec![],
        }
    }
    fn style(&mut self, cell: &slate_core::terminal::Cell) -> usize {
        let flags =
            u8::from(cell.bold) | u8::from(cell.italic) << 1 | u8::from(cell.underline) << 2;
        match self
            .styles
            .iter()
            .position(|(fg, bg, f)| *fg == cell.fg && *bg == cell.bg && *f == flags)
        {
            Some(index) => index,
            None => {
                self.styles.push((cell.fg.clone(), cell.bg.clone(), flags));
                self.styles.len() - 1
            }
        }
    }
    fn push_row(&mut self, row: usize, cells: &[slate_core::terminal::Cell]) {
        let mut runs: Vec<(usize, usize, String)> = vec![];
        let mut col = 0;
        while let Some(cell) = cells.get(col) {
            let style = self.style(cell);
            let plain = !cell.wide
                && !cell.continuation
                && (cell.text.is_empty() || cell.text.len() == 1 && cell.text.is_ascii());
            if plain {
                let glyph = if cell.text.is_empty() {
                    " "
                } else {
                    &cell.text
                };
                match runs.last_mut() {
                    Some((last, columns, text))
                        if *last == style && text.len() == *columns && text.is_ascii() =>
                    {
                        text.push_str(glyph);
                        *columns += 1;
                    }
                    _ => runs.push((style, 1, glyph.into())),
                }
                col += 1;
            } else {
                let columns = if cell.wide && cells.get(col + 1).is_some_and(|c| c.continuation) {
                    2
                } else {
                    1
                };
                let text = if cell.continuation {
                    String::new()
                } else {
                    cell.text.clone()
                };
                runs.push((style, columns, text));
                col += columns;
            }
        }
        self.lines.push(TerminalRow { row, runs });
    }
}
#[derive(Serialize)]
struct GuiUpdate {
    #[serde(flatten)]
    frame: serde_json::Map<String, serde_json::Value>,
    surfaces: Vec<SurfacePatch>,
}
/// Run the graphical interface. `qt_arguments` are Qt's own options
/// (-platform, -style, -reverse…), which Qt consumes.
pub fn run(app: App) -> i32 {
    run_with(app, &[])
}
pub fn run_with(mut app: App, qt_arguments: &[String]) -> i32 {
    app.terminal_frontend = false;
    app.refresh();
    // A desktop session authorizes privileged saves through polkit.
    if slate_core::fsio::which("pkexec").is_some() {
        app.elevation_mode = slate_core::ElevationMode::Background("pkexec");
    }
    let mut context = GuiContext::new(app);
    let mut arguments: Vec<CString> = std::iter::once("slate-gui".to_string())
        .chain(qt_arguments.iter().cloned())
        .filter_map(|a| CString::new(a).ok())
        .collect();
    let mut pointers: Vec<*mut c_char> = arguments
        .iter_mut()
        .map(|a| a.as_ptr() as *mut c_char)
        .chain(std::iter::once(std::ptr::null_mut()))
        .collect();
    // Terminated while the workspace was restored: never open the window.
    let code = if terminated() {
        0
    } else {
        unsafe {
            slate_qt_run(
                (&mut context as *mut GuiContext).cast(),
                arguments.len() as i32,
                pointers.as_mut_ptr(),
            )
        }
    };
    drop(arguments);
    if terminated() && !context.app.wants_recovery() {
        let saved = context.app.emergency_save();
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
    if code == 0 && !terminated() {
        // The caller may read its files as soon as completion is delivered.
        if let Err(error) = context.app.flush_workspace() {
            context
                .app
                .finish_editor_waits(Some(&format!("Cannot finish editor session: {error:#}")));
            eprintln!("Cannot finish editor session: {error:#}");
            return 1;
        }
        context.app.finish_editor_waits(
            (!context.app.quit).then_some("GUI closed before editing completed"),
        );
    } else {
        context
            .app
            .finish_editor_waits(Some("GUI terminated before editing completed"));
    }
    code
}
struct Target(*mut c_void);
unsafe impl Send for Target {}
unsafe impl Sync for Target {}
/// Register a thread-safe wake callback (platforms without an event fd).
#[no_mangle]
unsafe extern "C" fn slate_set_waker(
    context: *mut c_void,
    wake: Option<unsafe extern "C" fn(*mut c_void)>,
    target: *mut c_void,
) {
    let (Some(wake), false) = (wake, context.is_null()) else {
        return;
    };
    let target = Target(target);
    (&*context.cast::<GuiContext>())
        .app
        .events()
        .set_waker(move || {
            let target = &target;
            unsafe { wake(target.0) }
        });
}
#[no_mangle]
unsafe extern "C" fn slate_event_fd(context: *mut c_void) -> i32 {
    #[cfg(unix)]
    {
        (&*context.cast::<GuiContext>()).app.events().fd()
    }
    #[cfg(not(unix))]
    {
        let _ = context;
        -1
    }
}
#[no_mangle]
unsafe extern "C" fn slate_request(context: *mut c_void, request: *const c_char) -> *mut c_char {
    if context.is_null() || request.is_null() {
        return CString::new("{}").unwrap().into_raw();
    }
    let response = catch_unwind(AssertUnwindSafe(|| {
        let state = &mut *context.cast::<GuiContext>();
        let request: serde_json::Value =
            serde_json::from_slice(CStr::from_ptr(request).to_bytes()).unwrap_or_default();
        respond(state, request)
    }))
    .unwrap_or_else(|_| {
        // The frontend's cached projection may no longer match the core;
        // resend everything on the next update.
        let state = &mut *context.cast::<GuiContext>();
        state.revision = 0;
        state.viewport.clear();
        state.panes.clear();
        state.editors.clear();
        state.terminals.clear();
        // The interrupted action may have left the core half-updated. Keep
        // working (the user must be able to save), but copy unsaved text to
        // NAME.save files now and stop checkpoints, which could replace the
        // last consistent recovery state with an inconsistent one.
        let _ = catch_unwind(AssertUnwindSafe(|| {
            state
                .app
                .quarantine("An internal error interrupted the last action")
        }));
        "{\"status\":\"Internal error processing GUI request\"}".into()
    });
    // JSON escapes NUL characters, so the response never contains one.
    CString::new(response)
        .unwrap_or_else(|_| CString::new("{}").unwrap())
        .into_raw()
}
// Resolve every user command before dispatch. Qt-specific confirmations live
// here, after Rust has interpreted configured shortcuts and palette commands.
fn resolve_gui_command(app: &App, request: &serde_json::Value) -> anyhow::Result<Command> {
    if request["action"] == "paste" && !request["text"].is_string() {
        return Ok(Command::Paste {
            text: app.clipboard.clone(),
        });
    }
    if request["action"] == "shortcut" {
        let chord = request["chord"].as_str().unwrap_or_default();
        let binding = app
            .preferences
            .global_keys
            .get(chord)
            .ok_or_else(|| anyhow::anyhow!("Unknown shortcut: {chord}"))?;
        return app.resolve_command_input(binding);
    }
    if request["action"] == "command" {
        return app.resolve_command_input(request["text"].as_str().unwrap_or_default());
    }
    let command: Command = serde_json::from_value(request.clone())?;
    match command {
        Command::Key { ref key } => {
            if let Some(binding) = app.key_binding(key) {
                return app.resolve_command_input(binding);
            }
            Ok(command)
        }
        Command::InvokeAction { id, argument } => app.resolve_action(&id, &argument),
        _ => Ok(command),
    }
}
fn dispatch_gui(state: &mut GuiContext, request: serde_json::Value) -> String {
    let app = &mut state.app;
    let result = (|| -> anyhow::Result<serde_json::Value> {
        let mut command = resolve_gui_command(app, &request)?;
        let id = command.action_id();
        if let Some(info) = id.as_deref().and_then(|id| app.command_info(id)) {
            anyhow::ensure!(info.enabled, "{}", info.reason);
        }
        // Force is a backend capability, not permission to skip a GUI dialog.
        // Dialog acceptance must explicitly mark the request as confirmed.
        let confirmation = match command {
            Command::Quit { force: false } if app.dirty() => Some("quit"),
            Command::Quit { force: true } => Some("discard-quit"),
            Command::CloseDocument { force: true } | Command::CloseTab { force: true, .. } => {
                Some("discard-document")
            }
            _ => None,
        };
        if request["confirmed"] != true {
            if let Some(id) = confirmation {
                return Ok(serde_json::json!({"confirmation":id}));
            }
        }
        if let Command::Paste { text } = &mut command {
            if !(request["action"] == "paste" && request["text"].is_string()) {
                let Some(input) = request["clipboard_input"].as_str() else {
                    return Ok(serde_json::json!({"clipboard_request":true}));
                };
                app.clipboard = input.to_owned();
                *text = input.to_owned();
            }
        }
        let apply_theme = matches!(command, Command::ReloadSettings)
            || matches!(&command, Command::Configure { name, value } if ["theme", "editor-theme", "terminal-theme"].contains(&name.as_str()) && value == "auto");
        app.dispatch(command);
        let mut response = serde_json::json!({"status":app.status,"quit":app.quit});
        if apply_theme {
            response["apply_theme"] = true.into();
        }
        let requests = app.take_frontend_requests();
        if !requests.is_empty() {
            response["requests"] = requests.into();
        }
        if id.as_deref().is_some_and(|id| id == "copy" || id == "cut") {
            response["clipboard"] = app.clipboard.clone().into();
        }
        Ok(response)
    })();
    match result {
        Ok(response) => response.to_string(),
        Err(error) => {
            app.status = format!("Error: {error:#}");
            state.viewport.clear();
            serde_json::json!({"status":app.status,"quit":app.quit}).to_string()
        }
    }
}
fn respond(state: &mut GuiContext, request: serde_json::Value) -> String {
    let action = request["action"].as_str().unwrap_or_default().to_owned();
    let app = &mut state.app;
    match action.as_str() {
        "file_dialog_context" => {
            let path = match app.layout.view(app.focus) {
                Some(slate_core::layout::View::Editor(id)) => app
                    .views
                    .get(id)
                    .and_then(|view| app.documents.get(&view.document))
                    .and_then(|document| document.path.as_ref()),
                _ => None,
            };
            serde_json::json!({"root": app.root, "path": path}).to_string()
        }
        "accessible_context" => app.accessible_context(request["pane"].as_u64().unwrap_or_default()).to_string(),
        "accessible_text" => serde_json::json!({"text": app.accessible_text(request["pane"].as_u64().unwrap_or_default(), request["start"].as_u64().unwrap_or_default() as usize, request["end"].as_u64().unwrap_or_default() as usize)}).to_string(),
        "accessible_position" => app.accessible_position(request["pane"].as_u64().unwrap_or_default(), request["offset"].as_u64().unwrap_or_default() as usize).to_string(),
        "accessible_offset" => serde_json::json!({"offset": app.accessible_offset(request["pane"].as_u64().unwrap_or_default(), request["row"].as_u64().unwrap_or_default() as usize, request["col"].as_u64().unwrap_or_default() as usize)}).to_string(),
        "input_context" => serde_json::json!({
            "editor": request["pane"].as_u64().and_then(|id| app.editor_input_context(id))
        })
        .to_string(),
        "overview" => serde_json::json!({
            "overview": request["pane"].as_u64().and_then(|id| app.pane_overview(id))
        })
        .to_string(),
        "document_text" => match app.active_document() {
            Some((title, text)) => serde_json::json!({"title": title, "text": text}).to_string(),
            None => "{}".into(),
        },
        // Session logout: keep unsaved work without asking.
        "flush" => {
            let result = if app.wants_recovery() {
                app.flush_workspace()
                    .map(|_| "Recovery state saved".to_string())
            } else {
                let saved = app.emergency_save();
                Ok(format!(
                    "Saved {} unsaved buffer(s) as .save files",
                    saved.len()
                ))
            };
            serde_json::json!({"status": result.unwrap_or_else(|e| format!("Error: {e:#}"))})
                .to_string()
        }
        "diagnostics" => {
            serde_json::json!({ "screen_builds": app.screen_builds, "revision": app.revision() })
                .to_string()
        }
        #[cfg(test)]
        "test_panic" => panic!("injected GUI request failure"),
        "catalog" => {
            let query = request["query"].as_str().unwrap_or_default();
            let pane = request["pane"].as_u64().unwrap_or(app.focus);
            let row = request["row"].as_u64().map(|row| row as usize);
            serde_json::json!({"commands": app.command_catalog_for(query, pane, row)}).to_string()
        }
        "snapshot" | "update" => {
            app.process_events();
            // Workers can notify while the projection is assembled. Record the
            // revision before reading their data so a later notification cannot
            // be mistaken for content already included in this update.
            let revision = app.revision();
            let viewport = request.to_string();
            let patch = action == "update";
            if patch && state.revision == revision && state.viewport == viewport {
                return "{\"unchanged\":true}".into();
            }
            let number = |key: &str, default: u16| {
                request[key]
                    .as_u64()
                    .map(|n| n.min(u16::MAX as u64) as u16)
                    .unwrap_or(default)
            };
            let header = number("header_height", 43);
            let cell = (number("cell_width", 9), number("cell_height", 18));
            let mut snapshot = app.gui_snapshot(
                slate_core::layout::Rect {
                    x: 0,
                    y: 0,
                    width: number("width", 1280),
                    height: number("height", 720),
                },
                6,
                cell,
                header,
                (
                    number("minimum_width", 160),
                    header.saturating_add(cell.1.saturating_mul(3)),
                ),
                patch.then_some((state.files, state.git)),
            );
            let files_changed = state.files != snapshot.files_revision;
            let git_changed = state.git != snapshot.git_revision;
            state.files = snapshot.files_revision;
            state.git = snapshot.git_revision;
            let colors = (snapshot.foreground.clone(), snapshot.background.clone());
            let same_colors = state.colors.as_ref() == Some(&colors);
            state.colors = Some(colors);
            let mut surfaces = Vec::new();
            let mut panes = BTreeMap::new();
            let mut editors = BTreeMap::new();
            let mut terminals = BTreeMap::new();
            for pane in &mut snapshot.panes {
                panes.insert(pane.id, pane.revision);
                if let Some(mut text) = pane.text.take() {
                    let prior = patch.then(|| state.editors.get(&pane.id)).flatten();
                    // While highlighting catches up, unchanged document lines
                    // keep their previous styles. Do not flash/re-layout the whole
                    // viewport in default colors on every edit.
                    if !text.highlighted && same_colors {
                        if let Some(old) =
                            prior.filter(|p| p.document == text.document && p.left == text.left)
                        {
                            for (row, line) in text.lines.iter_mut().enumerate() {
                                if let Some(previous) = (text.top + row)
                                    .checked_sub(old.top)
                                    .and_then(|r| old.lines.get(r))
                                {
                                    if previous.layout.text == line.layout.text
                                        && previous.layout.columns == line.layout.columns
                                    {
                                        line.layout.formats.clone_from(&previous.layout.formats);
                                    }
                                }
                            }
                        }
                    }
                    let mut lines = Vec::new();
                    for (row, line) in text.lines.iter().enumerate() {
                        let old = prior.and_then(|p| p.lines.get(row));
                        let layout = old
                            .filter(|p| p.layout == line.layout)
                            .is_none()
                            .then(|| line.layout.clone());
                        let overlays = old
                            .filter(|p| p.overlays == line.overlays)
                            .is_none()
                            .then(|| line.overlays.clone());
                        if layout.is_some() || overlays.is_some() {
                            lines.push(LinePatch {
                                row,
                                layout,
                                overlays,
                            });
                        }
                    }
                    if prior != Some(&text) || state.viewport != viewport {
                        surfaces.push(SurfacePatch {
                            id: pane.id,
                            kind: "editor",
                            rows: pane.rows,
                            cols: pane.cols,
                            cursor: text.cursor,
                            line_count: text.lines.len(),
                            lines,
                            screen: None,
                        });
                    }
                    editors.insert(pane.id, text);
                } else if let Some(screen) = pane.screen.take() {
                    if !patch || state.panes.get(&pane.id) != Some(&pane.revision) {
                        surfaces.push(SurfacePatch {
                            id: pane.id,
                            kind: "terminal",
                            rows: pane.rows,
                            cols: pane.cols,
                            cursor: screen.cursor,
                            line_count: 0,
                            lines: vec![],
                            screen: Some({
                                let mut compact = TerminalPatch::new(&screen);
                                for (row, cells) in screen.cells.iter().enumerate() {
                                    let unchanged = patch
                                        && state.terminals.get(&pane.id).is_some_and(|old| {
                                            old.cols == screen.cols
                                                && old.cells.get(row) == Some(cells)
                                        });
                                    if !unchanged {
                                        compact.push_row(row, cells);
                                    }
                                }
                                compact
                            }),
                        });
                    }
                    terminals.insert(pane.id, screen);
                }
            }
            state.panes = panes;
            state.editors = editors;
            state.terminals = terminals;
            state.revision = revision;
            state.viewport = viewport;
            // Only small metadata enters this temporary JSON value. Bulk surfaces
            // serialize directly and never become part of the QML frame.
            let mut frame = serde_json::to_value(snapshot)
                .unwrap()
                .as_object()
                .unwrap()
                .clone();
            frame.remove("clipboard");
            frame.remove("commands");
            if !patch || state.global_keys.as_ref() != Some(&app.preferences.global_keys) {
                frame.insert(
                    "global_shortcuts".into(),
                    serde_json::json!(app.preferences.global_keys.keys().collect::<Vec<_>>()),
                );
                state.global_keys = Some(app.preferences.global_keys.clone());
            }
            frame.insert("command_revision".into(), state.revision.into());
            let requests = app.take_frontend_requests();
            if !requests.is_empty() {
                frame.insert("requests".into(), requests.into());
            }
            if patch && !files_changed {
                frame.remove("files");
            }
            if patch && !git_changed {
                frame.remove("git");
            }
            if patch && frame.get("settings") == state.settings.as_ref() {
                frame.remove("settings");
            } else {
                state.settings = frame.get("settings").cloned();
            }
            serde_json::to_string(&GuiUpdate { frame, surfaces }).unwrap()
        }
        _ => dispatch_gui(state, request),
    }
}
#[no_mangle]
unsafe extern "C" fn slate_response_free(response: *mut c_char) {
    if !response.is_null() {
        drop(CString::from_raw(response));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn test_app(
        path: &std::path::Path,
        mode: Option<slate_core::preferences::StartupMode>,
    ) -> anyhow::Result<App> {
        static TEST_HOME: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
        TEST_HOME.get_or_init(|| {
            let dir = tempfile::tempdir().unwrap();
            std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
            std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
            std::env::set_var("SHELL", "/bin/sh");
            dir
        });
        App::new_with_startup(path, mode)
    }
    fn request(state: &mut GuiContext, value: serde_json::Value) -> serde_json::Value {
        let input = CString::new(value.to_string()).unwrap();
        unsafe {
            let response = slate_request((state as *mut GuiContext).cast(), input.as_ptr());
            let value = serde_json::from_slice(CStr::from_ptr(response).to_bytes()).unwrap();
            slate_response_free(response);
            value
        }
    }
    #[test]
    fn file_pane_requests_keep_the_gui_inside_the_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("workspace");
        let child = root.join("child");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(child.join("nested.txt"), "nested").unwrap();
        let mut app =
            test_app(&root, Some(slate_core::preferences::StartupMode::Workspace)).unwrap();
        app.terminal_frontend = false;
        app.refresh();
        let pane = app
            .layout
            .panes()
            .into_iter()
            .find(|p| matches!(app.layout.view(*p), Some(slate_core::layout::View::Files)))
            .unwrap();
        app.dispatch(Command::Focus { pane });
        let mut state = GuiContext::new(app);
        let snapshot = serde_json::json!({"action":"snapshot"});
        let update = serde_json::json!({"action":"update"});
        settle(&mut state, &update);
        let initial = request(&mut state, snapshot.clone());
        assert!(!initial["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == ".."));
        request(
            &mut state,
            serde_json::json!({
                "action":"action", "name":"expand-folder", "argument":child,
            }),
        );
        settle(&mut state, &update);
        let expanded = request(&mut state, snapshot.clone());
        assert!(expanded["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == "nested.txt"));
        for action in [
            serde_json::json!({"action":"browse", "path":".."}),
            serde_json::json!({"action":"invoke_action", "id":"toggle-folder", "argument":dir.path()}),
        ] {
            let response = request(&mut state, action);
            assert!(response["status"]
                .as_str()
                .unwrap()
                .contains("outside the workspace"));
            let frame = request(&mut state, snapshot.clone());
            assert_eq!(frame["browser"], root.to_string_lossy().as_ref());
            assert!(!frame["files"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["name"] == ".."));
        }
    }
    #[test]
    fn idle_and_component_updates_do_not_transport_unchanged_grids() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("edit.txt");
        std::fs::write(&file, "hello\n").unwrap();
        let mut state = GuiContext::new(
            test_app(&file, Some(slate_core::preferences::StartupMode::Workspace)).unwrap(),
        );
        let update = serde_json::json!({"action":"update", "width":1280, "height":720});
        let initial = request(&mut state, update.clone());
        assert!(initial["surfaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["kind"] == "editor" && p.get("lines").is_some()));
        // Wait for background startup to settle before asserting idle behavior.
        settle(&mut state, &update);
        let idle = request(&mut state, update.clone());
        assert_eq!(idle, serde_json::json!({"unchanged":true}));
        state.app.dispatch(Command::Key {
            key: slate_core::Key {
                key: "Right".into(),
                ..Default::default()
            },
        });
        let patch = request(&mut state, update.clone());
        assert!(patch.get("files").is_none());
        assert!(patch.get("git").is_none());
        let surfaces = patch["surfaces"].as_array().unwrap();
        let editor = surfaces.iter().find(|p| p["kind"] == "editor").unwrap();
        assert!(
            editor.get("lines").is_none(),
            "Cursor movement must not resend text layouts"
        );
        assert!(surfaces.iter().all(|p| p["kind"] != "terminal"));
        assert!(patch["panes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p.get("screen").is_none() && p.get("text").is_none()));
        let focus_only = request(&mut state, serde_json::json!({"action":"focus", "pane":1}));
        assert!(focus_only.get("status").is_some());
        let patch = request(&mut state, update);
        assert!(patch["surfaces"].as_array().unwrap().is_empty());
        let error = request(&mut state, serde_json::json!({"action":"invalid-action"}));
        assert!(error["status"].as_str().unwrap().starts_with("Error:"));
        let patch = request(
            &mut state,
            serde_json::json!({"action":"update", "width":1280, "height":720}),
        );
        assert!(patch["status"].as_str().unwrap().starts_with("Error:"));
    }
    fn editor_state(file: &std::path::Path) -> GuiContext {
        let mut app =
            test_app(file, Some(slate_core::preferences::StartupMode::EditorOnly)).unwrap();
        app.preferences = Default::default();
        GuiContext::new(app)
    }
    fn settle(state: &mut GuiContext, update: &serde_json::Value) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut quiet = 0;
        while quiet < 3 {
            let frame = request(state, update.clone());
            quiet = if frame["unchanged"] == true {
                quiet + 1
            } else {
                0
            };
            assert!(
                std::time::Instant::now() < deadline,
                "Background work did not settle"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    #[test]
    fn large_viewport_is_compact_and_edits_only_patch_the_changed_line() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("edit.txt");
        std::fs::write(&file, "first\nsecond\nthird").unwrap();
        let mut state = editor_state(&file);
        state.app.clipboard = "clipboard".repeat(100_000);
        let update = serde_json::json!({"action":"update","width":3840,"height":2160});
        let initial = request(&mut state, update.clone());
        assert!(
            initial.to_string().len() < 32_000,
            "Short files must not scale to viewport-sized JSON"
        );
        assert!(initial.get("clipboard").is_none() && initial.get("commands").is_none());
        assert_eq!(initial["surfaces"][0]["line_count"], 3);
        settle(&mut state, &update);
        request(
            &mut state,
            serde_json::json!({"action":"key","key":"x","text":"x"}),
        );
        let changed = request(&mut state, update.clone());
        let lines = changed["surfaces"][0]["lines"].as_array().unwrap();
        assert_eq!(
            lines.len(),
            1,
            "Unchanged lines must retain styles while highlighting catches up"
        );
        assert_eq!(lines[0]["row"], 0);
        assert_eq!(lines[0]["layout"]["text"], "   1  xfirst");
        assert!(changed.get("settings").is_none());
        settle(&mut state, &update);
        request(
            &mut state,
            serde_json::json!({"action":"key","key":"Right","shift":true}),
        );
        let selected = request(&mut state, update);
        let lines = selected["surfaces"][0]["lines"].as_array().unwrap();
        assert!(lines.iter().all(|line| line.get("layout").is_none()));
        assert!(lines.iter().any(|line| line.get("overlays").is_some()));
    }
    #[test]
    fn input_methods_observe_edits_and_active_tabs_before_the_render_flush() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("edit.txt");
        std::fs::write(&file, "original").unwrap();
        let mut state = editor_state(&file);
        let update = serde_json::json!({"action":"update"});
        request(&mut state, update.clone());
        let rendered_revision = state.revision;
        request(&mut state, serde_json::json!({"action":"new"}));
        request(
            &mut state,
            serde_json::json!({"action":"input_method","text":"猫 👩‍💻"}),
        );
        let context = request(
            &mut state,
            serde_json::json!({"action":"input_context","pane":2}),
        );
        assert_eq!(context["editor"]["surrounding"], "猫 👩‍💻");
        assert_eq!(context["editor"]["cursor"], 7);
        assert_eq!(
            state.revision, rendered_revision,
            "Queries must not flush rendering"
        );
        request(&mut state, serde_json::json!({"action":"select_all"}));
        let context = request(
            &mut state,
            serde_json::json!({"action":"input_context","pane":2}),
        );
        assert_eq!(context["editor"]["anchor"], 0);
        assert_eq!(context["editor"]["selection"], "猫 👩‍💻");
        request(
            &mut state,
            serde_json::json!({"action":"switch_tab","pane":2,"index":0}),
        );
        let context = request(
            &mut state,
            serde_json::json!({"action":"input_context","pane":2}),
        );
        assert_eq!(context["editor"]["surrounding"], "original");
        let invalid = request(
            &mut state,
            serde_json::json!({"action":"input_context","pane":999}),
        );
        assert!(invalid["editor"].is_null());
    }
    #[test]
    fn desktop_requests_overview_print_and_session_flush() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("doc.txt");
        std::fs::write(&file, "one\n    two\n").unwrap();
        let mut app = test_app(&file, None).unwrap();
        app.terminal_frontend = false;
        let mut state = GuiContext::new(app);
        request(
            &mut state,
            serde_json::json!({"action":"update","width":1280,"height":720}),
        );
        let pane = state.app.focus;
        let overview = request(
            &mut state,
            serde_json::json!({"action":"overview","pane":pane}),
        );
        assert_eq!(overview["overview"]["total"], 3);
        assert_eq!(overview["overview"]["lines"][1], serde_json::json!([4, 7]));
        let document = request(&mut state, serde_json::json!({"action":"document_text"}));
        assert_eq!(document["title"], "doc.txt");
        assert_eq!(document["text"], "one\n    two\n");
        // Print and new-window are carried out by the Qt side.
        let printed = request(
            &mut state,
            serde_json::json!({"action":"invoke_action","id":"print","argument":""}),
        );
        assert_eq!(printed["requests"], serde_json::json!(["print"]));
        // Logging out keeps unsaved work: single files get NAME.save.
        request(
            &mut state,
            serde_json::json!({"action":"paste","text":"unsaved "}),
        );
        let flushed = request(&mut state, serde_json::json!({"action":"flush"}));
        assert!(
            flushed["status"].as_str().unwrap().contains(".save"),
            "{flushed}"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("doc.txt.save")).unwrap(),
            "unsaved one\n    two\n"
        );
    }
    #[test]
    fn null_requests_are_rejected_without_touching_state() {
        unsafe {
            let response = slate_request(std::ptr::null_mut(), std::ptr::null());
            assert_eq!(CStr::from_ptr(response).to_str().unwrap(), "{}");
            slate_response_free(response);
            slate_set_waker(std::ptr::null_mut(), None, std::ptr::null_mut());
        }
    }
    #[test]
    fn terminal_output_finishes_in_the_cached_view_without_another_user_event() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = GuiContext::new(
            test_app(
                dir.path(),
                Some(slate_core::preferences::StartupMode::Workspace),
            )
            .unwrap(),
        );
        let update = serde_json::json!({"action":"update","width":3840,"height":2160});
        settle(&mut state, &update);
        request(&mut state, serde_json::json!({"action":"focus","pane":3}));
        // Split the marker in the printf arguments so echoed shell input cannot
        // satisfy the assertion before the command actually reaches its end.
        request(
            &mut state,
            serde_json::json!({"action":"paste","text":
                "i=0; while [ \"$i\" -lt 80 ]; do printf '%s\\n' \"$i\"; i=$((i+1)); sleep .001; done; printf '__%s__' TAIL_FINISHED"
            }),
        );
        // Shells with bracketed paste treat a pasted line break as text; Enter runs it.
        request(
            &mut state,
            serde_json::json!({"action":"key","key":"Enter"}),
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            request(&mut state, update.clone());
            let finished = state.terminals.values().any(|screen| {
                screen.cells.iter().any(|row| {
                    row.iter()
                        .map(|cell| cell.text.as_str())
                        .collect::<String>()
                        .contains("__TAIL_FINISHED__")
                })
            });
            if finished {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "The GUI skipped the final terminal output notification"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    #[test]
    fn terminal_selection_only_transports_its_changed_row_and_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = GuiContext::new(
            test_app(
                dir.path(),
                Some(slate_core::preferences::StartupMode::Workspace),
            )
            .unwrap(),
        );
        let update = serde_json::json!({"action":"update","width":1360,"height":820});
        settle(&mut state, &update);
        request(
            &mut state,
            serde_json::json!({"action":"pointer","pane":3,"row":0,"col":0,"kind":"press","button":0}),
        );
        let drag = serde_json::json!({"action":"pointer","pane":3,"row":0,"col":1,"kind":"drag","button":0});
        request(&mut state, drag.clone());
        let selected = request(&mut state, update.clone());
        let surface = selected["surfaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|surface| surface["kind"] == "terminal")
            .unwrap();
        assert_eq!(surface["screen"]["lines"].as_array().unwrap().len(), 1);
        assert_eq!(surface["screen"]["lines"][0]["row"], 0);
        assert!(surface["cursor"].is_null());
        request(&mut state, drag);
        let unchanged = request(&mut state, update);
        assert!(
            unchanged["unchanged"] == true
                || unchanged["surfaces"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|surface| {
                        surface["kind"] != "terminal"
                            || surface["screen"]["lines"].as_array().unwrap().is_empty()
                    })
        );
    }
    #[test]
    fn clipboard_handshake_respects_custom_bindings_and_does_not_echo_clipboard_on_typing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("edit.txt");
        std::fs::write(&file, "old").unwrap();
        let mut state = editor_state(&file);
        state
            .app
            .preferences
            .editor_keys
            .insert("f5".into(), "paste".into());
        request(
            &mut state,
            serde_json::json!({"action":"key","key":"a","ctrl":true}),
        );
        let paste = serde_json::json!({"action":"key","key":"f5"});
        assert_eq!(
            request(&mut state, paste.clone()),
            serde_json::json!({"clipboard_request":true})
        );
        assert_eq!(state.app.documents[&10].text(), "old");
        let mut paste = paste;
        paste["clipboard_input"] = "猫 pasted".into();
        request(&mut state, paste);
        assert_eq!(state.app.documents[&10].text(), "猫 pasted");
        request(
            &mut state,
            serde_json::json!({"action":"key","key":"a","ctrl":true}),
        );
        let copied = request(
            &mut state,
            serde_json::json!({"action":"key","key":"c","ctrl":true}),
        );
        assert_eq!(copied["clipboard"], "猫 pasted");
        let typed = request(
            &mut state,
            serde_json::json!({"action":"key","key":"x","text":"x"}),
        );
        assert!(typed.get("clipboard").is_none());
        let save = request(
            &mut state,
            serde_json::json!({"action":"key","key":"s","ctrl":true}),
        );
        assert!(save.get("clipboard_request").is_none() && save.get("clipboard").is_none());
    }
    #[test]
    fn quit_and_discard_routes_require_confirmation_after_shortcut_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("edit.txt");
        std::fs::write(&file, "original").unwrap();
        let mut state = editor_state(&file);
        request(
            &mut state,
            serde_json::json!({"action":"paste","text":"DIRTY"}),
        );
        state
            .app
            .preferences
            .global_keys
            .insert("f5".into(), "quit".into());
        state
            .app
            .preferences
            .editor_keys
            .insert("f4".into(), "discard-document".into());
        for input in [
            serde_json::json!({"action":"quit","force":false}),
            serde_json::json!({"action":"invoke_action","id":"quit"}),
            serde_json::json!({"action":"command","text":"quit"}),
            serde_json::json!({"action":"key","key":"q","ctrl":true}),
            serde_json::json!({"action":"key","key":"f5"}),
            serde_json::json!({"action":"shortcut","chord":"f5"}),
        ] {
            assert_eq!(
                request(&mut state, input),
                serde_json::json!({"confirmation":"quit"})
            );
            assert!(!state.app.quit);
            assert_eq!(state.app.documents[&10].text(), "DIRTYoriginal");
        }
        for input in [
            serde_json::json!({"action":"command","text":"discard-quit"}),
            serde_json::json!({"action":"quit","force":true}),
        ] {
            assert_eq!(request(&mut state, input)["confirmation"], "discard-quit");
            assert!(!state.app.quit && state.app.dirty());
        }
        let pane = state.app.focus;
        let Some(slate_core::layout::View::Editor(view)) = state.app.layout.view(pane).cloned()
        else {
            panic!("an editor is focused");
        };
        for input in [
            serde_json::json!({"action":"invoke_action","id":"discard-document"}),
            serde_json::json!({"action":"command","text":"discard-document"}),
            serde_json::json!({"action":"key","key":"f4"}),
            serde_json::json!({"action":"close_tab","pane":pane,"view":view,"force":true}),
        ] {
            assert_eq!(
                request(&mut state, input)["confirmation"],
                "discard-document"
            );
            assert_eq!(state.app.documents[&10].text(), "DIRTYoriginal");
        }
        request(
            &mut state,
            serde_json::json!({"action":"invoke_action","id":"discard-document","confirmed":true}),
        );
        assert!(!state.app.dirty());
        assert_eq!(std::fs::read_to_string(file).unwrap(), "original");
        request(
            &mut state,
            serde_json::json!({"action":"quit","force":false}),
        );
        assert!(state.app.quit);
    }
    #[test]
    fn settings_and_disabled_clipboard_actions_use_the_same_context_on_every_route() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("edit.txt");
        std::fs::write(&file, "original").unwrap();
        let mut state = editor_state(&file);
        state
            .app
            .preferences
            .global_keys
            .insert("f2".into(), "settings".into());
        for input in [
            serde_json::json!({"action":"invoke_action","id":"settings"}),
            serde_json::json!({"action":"command","text":"settings"}),
            serde_json::json!({"action":"key","key":"f2"}),
            serde_json::json!({"action":"shortcut","chord":"f2"}),
        ] {
            request(&mut state, input);
            assert_eq!(state.app.prompt.as_ref().unwrap().kind, "settings");
            request(&mut state, serde_json::json!({"action":"dismiss_prompt"}));
        }
        request(&mut state, serde_json::json!({"action":"focus","pane":1}));
        for input in [
            serde_json::json!({"action":"invoke_action","id":"paste"}),
            serde_json::json!({"action":"command","text":"paste"}),
            serde_json::json!({"action":"paste"}),
            serde_json::json!({"action":"paste","text":"UNEXPECTED"}),
        ] {
            let response = request(&mut state, input);
            assert!(response["status"].as_str().unwrap().contains("Focus"));
            assert!(response.get("clipboard_request").is_none());
            assert_eq!(state.app.documents[&10].text(), "original");
        }
        request(&mut state, serde_json::json!({"action":"focus","pane":2}));
        assert_eq!(
            request(&mut state, serde_json::json!({"action":"paste"})),
            serde_json::json!({"clipboard_request":true})
        );
    }
    #[test]
    fn configured_global_shortcuts_are_revisioned_without_repeating_them_in_patches() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("edit.txt");
        std::fs::write(&file, "original").unwrap();
        let mut state = editor_state(&file);
        let update = serde_json::json!({"action":"update"});
        let initial = request(&mut state, update.clone());
        assert!(initial["global_shortcuts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|key| key == "Ctrl+q"));
        request(
            &mut state,
            serde_json::json!({"action":"key","key":"Right"}),
        );
        assert!(request(&mut state, update.clone())
            .get("global_shortcuts")
            .is_none());
        state
            .app
            .preferences
            .global_keys
            .insert("f5".into(), "quit".into());
        state.app.dispatch(Command::Focus { pane: 2 });
        let changed = request(&mut state, update);
        assert!(changed["global_shortcuts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|key| key == "f5"));
    }
    #[test]
    fn pending_saves_block_all_quit_routes_before_confirmation() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("edit.txt");
        std::fs::write(&file, "original").unwrap();
        let mut state = editor_state(&file);
        request(
            &mut state,
            serde_json::json!({"action":"paste","text":"EDIT"}),
        );
        request(&mut state, serde_json::json!({"action":"save"}));
        for input in [
            serde_json::json!({"action":"quit","force":false}),
            serde_json::json!({"action":"quit","force":true,"confirmed":true}),
            serde_json::json!({"action":"invoke_action","id":"quit"}),
            serde_json::json!({"action":"command","text":"discard-quit"}),
            serde_json::json!({"action":"key","key":"q","ctrl":true}),
        ] {
            let response = request(&mut state, input);
            assert!(response["status"]
                .as_str()
                .unwrap()
                .contains("pending file saves"));
            assert!(response.get("confirmation").is_none());
            assert!(!state.app.quit);
        }
    }
    #[test]
    fn accepting_quit_confirmation_discards_edits_without_writing_them() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("edit.txt");
        std::fs::write(&file, "original").unwrap();
        let mut state = editor_state(&file);
        request(
            &mut state,
            serde_json::json!({"action":"paste","text":"DIRTY"}),
        );
        assert_eq!(
            request(
                &mut state,
                serde_json::json!({"action":"quit","force":false})
            )["confirmation"],
            "quit"
        );
        request(
            &mut state,
            serde_json::json!({"action":"quit","force":true,"confirmed":true}),
        );
        assert!(state.app.quit && !state.app.dirty());
        assert_eq!(std::fs::read_to_string(file).unwrap(), "original");
    }
    #[test]
    fn bare_argument_actions_open_identical_prompts_from_palette_and_shortcuts() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("edit.txt");
        std::fs::write(&file, "original").unwrap();
        let mut state = editor_state(&file);
        state
            .app
            .preferences
            .editor_keys
            .insert("f4".into(), "layout-save".into());
        for input in [
            serde_json::json!({"action":"invoke_action","id":"layout-save"}),
            serde_json::json!({"action":"command","text":"layout-save"}),
            serde_json::json!({"action":"key","key":"f4"}),
        ] {
            request(&mut state, input);
            assert_eq!(state.app.prompt.as_ref().unwrap().kind, "layout-save");
            request(&mut state, serde_json::json!({"action":"dismiss_prompt"}));
        }
    }
    /// A workspace with `panic.txt` open, "BEFORE_PANIC " typed and
    /// checkpointed, then one injected request failure.
    fn quarantined_workspace(root: &std::path::Path) -> (GuiContext, serde_json::Value) {
        std::fs::create_dir_all(root).unwrap();
        std::fs::write(root.join("panic.txt"), "hello\n").unwrap();
        let mut state = GuiContext::new(
            test_app(root, Some(slate_core::preferences::StartupMode::Workspace)).unwrap(),
        );
        state.app.enable_workspace(false).unwrap();
        assert!(state.app.wants_recovery());
        let update = serde_json::json!({"action":"update","width":1280,"height":720});
        request(&mut state, update.clone());
        request(
            &mut state,
            serde_json::json!({"action":"command","text":"open panic.txt"}),
        );
        settle(&mut state, &update);
        request(
            &mut state,
            serde_json::json!({"action":"paste","text":"BEFORE_PANIC "}),
        );
        state.app.flush_workspace().unwrap();
        assert!(checkpoints(root).iter().any(|c| c.contains("BEFORE_PANIC")));
        let response = request(&mut state, serde_json::json!({"action":"test_panic"}));
        assert_eq!(response["status"], "Internal error processing GUI request");
        (state, update)
    }
    /// This workspace's live recovery checkpoint, if any.
    fn checkpoints(root: &std::path::Path) -> Vec<String> {
        let state = std::path::PathBuf::from(std::env::var_os("XDG_STATE_HOME").unwrap());
        let mut found = vec![];
        for entry in std::fs::read_dir(state.join("slate/workspaces")).unwrap() {
            let dir = entry.unwrap().path();
            let Ok(session) = std::fs::read_to_string(dir.join("session.json")) else {
                continue;
            };
            if session.contains(root.to_str().unwrap()) {
                found.push(session);
            }
        }
        found
    }
    fn set_aside(root: &std::path::Path) -> Vec<String> {
        let state = std::path::PathBuf::from(std::env::var_os("XDG_STATE_HOME").unwrap());
        let mut found = vec![];
        for entry in std::fs::read_dir(state.join("slate/workspaces")).unwrap() {
            for file in std::fs::read_dir(entry.unwrap().path()).unwrap() {
                let path = file.unwrap().path();
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                if name.starts_with("session.damaged-") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    if text.contains(root.to_str().unwrap()) {
                        found.push(text);
                    }
                }
            }
        }
        found
    }
    fn save_copies(root: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(root)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".save"))
            .collect();
        names.sort();
        names
    }
    #[test]
    fn interrupted_requests_refresh_one_copy_and_exit_never_restores_stale_buffers() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("workspace");
        let file = root.join("panic.txt");
        let (mut state, update) = quarantined_workspace(&root);
        // Unsaved text was copied without trusting the rest of the state.
        assert_eq!(
            std::fs::read_to_string(root.join("panic.txt.save")).unwrap(),
            "BEFORE_PANIC hello\n"
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello\n");
        // The session keeps running with a persistent error.
        request(
            &mut state,
            serde_json::json!({"action":"paste","text":"AFTER_PANIC "}),
        );
        request(
            &mut state,
            serde_json::json!({"action":"key","key":"Right"}),
        );
        let frame = request(&mut state, update.clone());
        let status = frame["status"].as_str().unwrap();
        assert!(
            status.starts_with("Error: An internal error interrupted the last action.")
                && status.contains("panic.txt.save")
                && status.contains("save your documents and restart Slate"),
            "{status}"
        );
        // Repeated failures refresh the same copy instead of adding files.
        for _ in 0..3 {
            request(&mut state, serde_json::json!({"action":"test_panic"}));
        }
        assert_eq!(save_copies(&root), ["panic.txt.save"]);
        assert_eq!(
            std::fs::read_to_string(root.join("panic.txt.save")).unwrap(),
            "BEFORE_PANIC AFTER_PANIC hello\n"
        );
        // Checkpoints keep the last consistent state while buffers are unsaved.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        request(&mut state, update.clone());
        let live = checkpoints(&root);
        assert!(live.iter().any(|c| c.contains("BEFORE_PANIC")));
        assert!(!live.iter().any(|c| c.contains("AFTER_PANIC")));
        // Termination and exit (single-file NAME.save and the workspace
        // flush) refresh the copy with the latest text, still one file.
        request(
            &mut state,
            serde_json::json!({"action":"paste","text":"LATEST "}),
        );
        state.app.emergency_save();
        state.app.flush_workspace().unwrap();
        assert_eq!(save_copies(&root), ["panic.txt.save"]);
        assert_eq!(
            std::fs::read_to_string(root.join("panic.txt.save")).unwrap(),
            "BEFORE_PANIC AFTER_PANIC hLATEST ello\n"
        );
        // With every buffer copied, the stale checkpoint is set aside rather
        // than restored at the next start.
        assert!(checkpoints(&root).is_empty());
        assert!(set_aside(&root).iter().any(|c| c.contains("BEFORE_PANIC")));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello\n");
    }
    #[test]
    fn saving_everything_after_an_interrupted_request_retires_the_stale_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("workspace");
        let file = root.join("panic.txt");
        let (mut state, update) = quarantined_workspace(&root);
        request(
            &mut state,
            serde_json::json!({"action":"paste","text":"SAVED "}),
        );
        request(
            &mut state,
            serde_json::json!({"action":"invoke_action","id":"save"}),
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !checkpoints(&root).is_empty() {
            assert!(
                std::time::Instant::now() < deadline,
                "Stale checkpoint kept"
            );
            request(&mut state, update.clone());
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "BEFORE_PANIC SAVED hello\n"
        );
        // A crash now restores nothing older than the saved file.
        assert!(set_aside(&root).iter().any(|c| c.contains("BEFORE_PANIC")));
        state.app.flush_workspace().unwrap();
        assert!(checkpoints(&root).is_empty());
    }
    #[test]
    fn terminal_rows_are_compact_runs_that_reproduce_every_cell() {
        use slate_core::terminal::{Cell, Screen};
        let styled = |text: &str, fg: &str, bold: bool| Cell {
            fg: fg.into(),
            bold,
            ..Cell::text(text)
        };
        let wide = |text: &str| Cell {
            wide: true,
            ..Cell::text(text)
        };
        let continuation = Cell {
            continuation: true,
            ..Cell::text("")
        };
        let row = vec![
            Cell::text("a"),
            Cell::text("b"),
            Cell::text(""),
            styled("c", "#ff0000", true),
            styled("d", "#ff0000", true),
            wide("猫"),
            continuation.clone(),
            Cell::text("e\u{301}"),
            wide("👩‍💻"),
            continuation,
            Cell {
                underline: true,
                italic: true,
                bg: "#000000".into(),
                ..Cell::text("")
            },
            Cell::text("<"),
        ];
        let screen = Screen {
            cells: vec![row.clone()],
            cursor: Some((0, 1)),
            rows: 1,
            cols: row.len() as u16,
        };
        let mut patch = TerminalPatch::new(&screen);
        patch.push_row(0, &row);
        let json = serde_json::to_value(&patch).unwrap();
        assert_eq!(
            json["lines"][0]["runs"],
            serde_json::json!([
                [0, 3, "ab "],
                [1, 2, "cd"],
                [0, 2, "猫"],
                [0, 1, "e\u{301}"],
                [0, 2, "👩‍💻"],
                [2, 1, " "],
                [0, 1, "<"]
            ])
        );
        // Every column keeps its text (blank cells are spaces, a wide glyph
        // covers its continuation) and its style.
        let styles = json["styles"].as_array().unwrap();
        let mut column = 0;
        for run in json["lines"][0]["runs"].as_array().unwrap() {
            let style = &styles[run[0].as_u64().unwrap() as usize];
            let columns = run[1].as_u64().unwrap() as usize;
            let text = run[2].as_str().unwrap();
            let plain = text.len() == columns && text.is_ascii();
            for offset in 0..columns {
                let cell = &row[column + offset];
                let expected = if cell.text.is_empty() && !cell.continuation {
                    " "
                } else {
                    cell.text.as_str()
                };
                if plain {
                    assert_eq!(&text[offset..offset + 1], expected);
                } else if offset == 0 {
                    assert_eq!(text, expected);
                } else {
                    assert!(cell.continuation);
                }
                if !cell.continuation {
                    let flags = u64::from(cell.bold)
                        | u64::from(cell.italic) << 1
                        | u64::from(cell.underline) << 2;
                    assert_eq!(style, &serde_json::json!([cell.fg, cell.bg, flags]));
                }
            }
            column += columns;
        }
        assert_eq!(column, row.len());

        // A full 240x60 screen of styled text: tens of kilobytes, not megabytes.
        let line: Vec<Cell> = (0..240)
            .map(|col| {
                let text = char::from(b'a' + (col % 26) as u8).to_string();
                styled(
                    &text,
                    if col / 10 % 2 == 0 {
                        "#d8dee9"
                    } else {
                        "#88c0d0"
                    },
                    false,
                )
            })
            .collect();
        let screen = Screen {
            cells: vec![line; 60],
            cursor: None,
            rows: 60,
            cols: 240,
        };
        let mut patch = TerminalPatch::new(&screen);
        for (row, cells) in screen.cells.iter().enumerate() {
            patch.push_row(row, cells);
        }
        let compact = serde_json::to_string(&patch).unwrap().len();
        let per_cell = serde_json::to_string(&screen.cells).unwrap().len();
        assert!(compact < 40_000, "{compact} bytes");
        assert!(compact * 20 < per_cell, "{compact} vs {per_cell} bytes");
    }
}
