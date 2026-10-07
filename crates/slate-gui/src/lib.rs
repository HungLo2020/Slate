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
    fn slate_qt_run(context: *mut c_void) -> i32;
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
#[derive(Serialize)]
struct TerminalRow {
    row: usize,
    cells: Vec<slate_core::terminal::Cell>,
}
#[derive(Serialize)]
struct TerminalPatch {
    rows: u16,
    cols: u16,
    cursor: Option<(u16, u16)>,
    lines: Vec<TerminalRow>,
}
#[derive(Serialize)]
struct GuiUpdate {
    #[serde(flatten)]
    frame: serde_json::Map<String, serde_json::Value>,
    surfaces: Vec<SurfacePatch>,
}
pub fn run(app: App) -> i32 {
    let mut context = GuiContext::new(app);
    unsafe { slate_qt_run((&mut context as *mut GuiContext).cast()) }
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
    let response = catch_unwind(AssertUnwindSafe(|| {
        let state = &mut *context.cast::<GuiContext>();
        let request: serde_json::Value =
            serde_json::from_slice(CStr::from_ptr(request).to_bytes()).unwrap_or_default();
        respond(state, request)
    }))
    .unwrap_or_else(|_| "{\"status\":\"Internal error processing GUI request\"}".into());
    CString::new(response).unwrap().into_raw()
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
        if let Some(info) = id.as_ref().and_then(|id| {
            app.command_catalog("")
                .into_iter()
                .find(|info| &info.id == id)
        }) {
            anyhow::ensure!(info.enabled, "{}", info.reason);
        }
        // Force is a backend capability, not permission to skip a GUI dialog.
        // Dialog acceptance must explicitly mark the request as confirmed.
        let confirmation = match command {
            Command::Quit { force: false } if app.dirty() => Some("quit"),
            Command::Quit { force: true } => Some("discard-quit"),
            Command::CloseDocument { force: true } => Some("discard-document"),
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
            || matches!(&command, Command::Configure { name, value } if name == "theme" && value == "auto");
        app.dispatch(command);
        let mut response = serde_json::json!({"status":app.status,"quit":app.quit});
        if apply_theme {
            response["apply_theme"] = true.into();
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
        "input_context" => serde_json::json!({
            "editor": request["pane"].as_u64().and_then(|id| app.editor_input_context(id))
        })
        .to_string(),
        "diagnostics" => {
            serde_json::json!({ "screen_builds": app.screen_builds, "revision": app.revision() })
                .to_string()
        }
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
                            screen: Some(TerminalPatch {
                                rows: screen.rows,
                                cols: screen.cols,
                                cursor: screen.cursor,
                                lines: screen
                                    .cells
                                    .iter()
                                    .enumerate()
                                    .filter_map(|(row, cells)| {
                                        let unchanged = patch
                                            && state.terminals.get(&pane.id).is_some_and(|old| {
                                                old.cols == screen.cols
                                                    && old.cells.get(row) == Some(cells)
                                            });
                                        (!unchanged).then(|| TerminalRow {
                                            row,
                                            cells: cells.clone(),
                                        })
                                    })
                                    .collect(),
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
    fn idle_and_component_updates_do_not_transport_unchanged_grids() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("edit.txt");
        std::fs::write(&file, "hello\n").unwrap();
        let mut state = GuiContext::new(
            App::new_with_startup(&file, Some(slate_core::preferences::StartupMode::Workspace))
                .unwrap(),
        );
        let update = serde_json::json!({"action":"update", "width":1280, "height":720});
        let initial = request(&mut state, update.clone());
        assert!(initial["surfaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["kind"] == "editor" && p.get("lines").is_some()));
        // Finish initial background work; the frontend isn't required to continuously render.
        std::thread::sleep(std::time::Duration::from_millis(350));
        request(&mut state, update.clone());
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
            App::new_with_startup(file, Some(slate_core::preferences::StartupMode::EditorOnly))
                .unwrap();
        app.preferences = Default::default();
        GuiContext::new(app)
    }
    fn settle(state: &mut GuiContext, update: &serde_json::Value) {
        request(state, update.clone());
        std::thread::sleep(std::time::Duration::from_millis(350));
        request(state, update.clone());
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
    fn terminal_output_finishes_in_the_cached_view_without_another_user_event() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = GuiContext::new(
            App::new_with_startup(
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
                "i=0; while [ \"$i\" -lt 80 ]; do printf '%s\\n' \"$i\"; i=$((i+1)); sleep .001; done; printf '__%s__' TAIL_FINISHED\r"
            }),
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
            App::new_with_startup(
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
        for input in [
            serde_json::json!({"action":"invoke_action","id":"discard-document"}),
            serde_json::json!({"action":"command","text":"discard-document"}),
            serde_json::json!({"action":"key","key":"f4"}),
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
}
