//! The bus between the editor and its IDE services. Language servers,
//! debug adapters and tasks run as child processes with reader threads;
//! everything they report arrives on the main loop as one of these typed
//! events, so the editor state is only ever changed in one place.
use serde_json::Value;

#[derive(Debug)]
pub enum Event {
    /// A JSON-RPC message from language server `server`.
    Lsp { server: u64, message: Value },
    /// A language server stopped.
    LspExited { server: u64, reason: String },
    /// A Debug Adapter Protocol message from the debug session.
    Dap { session: u64, message: Value },
    /// The debug adapter stopped.
    DapExited { session: u64, reason: String },
    /// Output from a running task.
    TaskOutput { task: u64, text: String },
    /// A task finished with an exit code (None when it could not start).
    TaskExited {
        task: u64,
        code: Option<i32>,
        error: String,
    },
}

impl crate::App {
    pub(crate) fn ide_event(&mut self, event: Event) {
        match event {
            Event::Lsp { .. } | Event::LspExited { .. } => self.lsp_event(event),
            Event::Dap { .. } | Event::DapExited { .. } => self.debug_event(event),
            Event::TaskOutput { .. } | Event::TaskExited { .. } => self.task_event(event),
        }
    }
}
