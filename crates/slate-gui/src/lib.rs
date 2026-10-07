use slate_core::{App, Command};
use std::{
    collections::BTreeMap,
    ffi::{c_char, c_void, CStr, CString},
    panic::{catch_unwind, AssertUnwindSafe},
};
extern "C" {
    fn slate_qt_run(context: *mut c_void) -> i32;
}
struct GuiContext {
    app: App,
    revision: u64,
    viewport: String,
    panes: BTreeMap<u64, u64>,
    files: u64,
    git: u64,
}
pub fn run(app: App) -> i32 {
    let mut context = GuiContext {
        app,
        revision: 0,
        viewport: String::new(),
        panes: BTreeMap::new(),
        files: 0,
        git: 0,
    };
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
fn respond(state: &mut GuiContext, request: serde_json::Value) -> String {
    let app = &mut state.app;
    match request["action"].as_str().unwrap_or_default() {
        "diagnostics" => serde_json::json!({
            "screen_builds": app.screen_builds,
            "revision": app.revision()
        })
        .to_string(),
        "catalog" => {
            let query = request["query"].as_str().unwrap_or_default();
            serde_json::json!({"commands": app.command_catalog(query)}).to_string()
        }
        "snapshot" | "update" => {
            app.process_events();
            let viewport = request.to_string();
            if request["action"] == "update"
                && state.revision == app.revision()
                && state.viewport == viewport
            {
                return "{\"unchanged\":true}".into();
            }
            let number = |key: &str, default: u16| {
                request[key]
                    .as_u64()
                    .map(|n| n.min(u16::MAX as u64) as u16)
                    .unwrap_or(default)
            };
            let header = number("header_height", 43);
            let revision = app.revision();
            let patch = request["action"] == "update";
            let area = slate_core::layout::Rect {
                x: 0,
                y: 0,
                width: number("width", 1280),
                height: number("height", 720),
            };
            let cell = (number("cell_width", 9), number("cell_height", 18));
            let minimum = (
                number("minimum_width", 160),
                header.saturating_add(cell.1.saturating_mul(3)),
            );
            let mut snapshot = app.snapshot_with_revisions(
                area,
                6,
                cell,
                header,
                minimum,
                patch.then_some((state.files, state.git)),
            );
            let files_changed = state.files != snapshot.files_revision;
            let git_changed = state.git != snapshot.git_revision;
            state.files = snapshot.files_revision;
            state.git = snapshot.git_revision;
            let mut panes = BTreeMap::new();
            for pane in &mut snapshot.panes {
                panes.insert(pane.id, pane.revision);
                if patch && state.panes.get(&pane.id) == Some(&pane.revision) {
                    pane.screen = None;
                }
            }
            state.panes = panes;
            if patch && !files_changed {
                snapshot.files.clear();
            }
            if patch && !git_changed {
                snapshot.git.clear();
            }
            state.revision = revision;
            state.viewport = viewport;
            let mut value = serde_json::to_value(snapshot).unwrap();
            if patch && !files_changed {
                value.as_object_mut().unwrap().remove("files");
            }
            if patch && !git_changed {
                value.as_object_mut().unwrap().remove("git");
            }
            value.to_string()
        }
        "command" => {
            app.command_line(request["text"].as_str().unwrap_or_default());
            serde_json::json!({"status": app.status, "quit": app.quit}).to_string()
        }
        _ => {
            match serde_json::from_value::<Command>(request) {
                Ok(command) => app.dispatch(command),
                Err(e) => {
                    app.status = format!("Invalid command: {e}");
                    state.viewport.clear();
                }
            }
            serde_json::json!({"status": app.status, "quit": app.quit, "clipboard": app.clipboard})
                .to_string()
        }
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
        let mut state = GuiContext {
            app: App::new(&file).unwrap(),
            revision: 0,
            viewport: String::new(),
            panes: BTreeMap::new(),
            files: 0,
            git: 0,
        };
        let update = serde_json::json!({"action":"update", "width":1280, "height":720});
        let initial = request(&mut state, update.clone());
        assert!(initial["panes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["kind"] == "editor" && p.get("screen").is_some()));
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
        let panes = patch["panes"].as_array().unwrap();
        assert!(panes
            .iter()
            .find(|p| p["kind"] == "editor")
            .unwrap()
            .get("screen")
            .is_some());
        assert!(panes
            .iter()
            .find(|p| p["kind"] == "terminal")
            .unwrap()
            .get("screen")
            .is_none());
        let focus_only = request(&mut state, serde_json::json!({"action":"focus", "pane":1}));
        assert!(focus_only.get("status").is_some());
        let patch = request(&mut state, update);
        assert!(patch["panes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p.get("screen").is_none()));
        let error = request(&mut state, serde_json::json!({"action":"invalid-action"}));
        assert!(error["status"]
            .as_str()
            .unwrap()
            .starts_with("Invalid command:"));
        let patch = request(
            &mut state,
            serde_json::json!({"action":"update", "width":1280, "height":720}),
        );
        assert!(patch["status"]
            .as_str()
            .unwrap()
            .starts_with("Invalid command:"));
    }
}
