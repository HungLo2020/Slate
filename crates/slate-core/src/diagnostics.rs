//! Bounded error history and a user-reviewable support report.
use crate::services::LatestWorker;
use crate::{
    user_state::{path as state, read},
    App,
};
use std::path::PathBuf;
#[derive(Default)]
pub(crate) struct Diagnostics {
    entries: Vec<String>,
    last: String,
    worker: Option<LatestWorker<(PathBuf, Vec<u8>)>>,
}
impl App {
    pub(crate) fn record_diagnostic_status(&mut self) {
        let status = &self.status;
        if ![
            "Error:",
            "Git:",
            "Search:",
            "Save failed:",
            "Settings error:",
            "Workspace recovery disabled:",
        ]
        .iter()
        .any(|p| status.starts_with(p))
            || self.diagnostics.last == *status
        {
            return;
        }
        if self.diagnostics.entries.is_empty() {
            self.diagnostics.entries = read("diagnostics.json");
            self.diagnostics.entries.truncate(100);
        }
        self.diagnostics.last = status.clone();
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let detail: String = status
            .chars()
            .take(1024)
            .filter(|c| *c == '\t' || !c.is_control())
            .collect();
        self.diagnostics.entries.push(format!("{time} {detail}"));
        if self.diagnostics.entries.len() > 100 {
            self.diagnostics.entries.remove(0);
        }
        if self.diagnostics.worker.is_none() {
            self.diagnostics.worker =
                Some(LatestWorker::new(|(path, bytes): (PathBuf, Vec<u8>)| {
                    let _ = crate::fsio::write_private(&path, &bytes);
                }));
        }
        if let Ok(bytes) = serde_json::to_vec(&self.diagnostics.entries) {
            self.diagnostics
                .worker
                .as_ref()
                .unwrap()
                .submit((state("diagnostics.json"), bytes));
        }
    }
    pub(crate) fn flush_diagnostics(&mut self) {
        if let Some(worker) = self.diagnostics.worker.take() {
            worker.finish();
        }
    }
    pub(crate) fn support_report(&mut self) {
        let mut text = format!("Slate {}\nPlatform: {} / {}\nInterface: {}\nConfiguration: {}\nState: {}\nWorkspace: {}\nProfile: {}\nOpen documents: {}\nPending saves: {}\nRecovery: {}\n\nRecent errors (timestamps are Unix seconds; paths and helper error messages may contain private information):\n",env!("CARGO_PKG_VERSION"),std::env::consts::OS,std::env::consts::ARCH,if self.terminal_frontend {"terminal"} else {"graphical"},crate::paths::config_dir().display(),crate::paths::state_dir().display(),self.root.display(),self.preferences.profile,self.documents.len(),self.saves.pending_save.len(),self.recovery_warning().unwrap_or(if self.recovery.store.is_some() {"enabled"} else {"disabled"}));
        let mut entries: Vec<String> = read("diagnostics.json");
        entries.extend(self.diagnostics.entries.iter().cloned());
        entries.dedup();
        entries.truncate(100);
        text.push_str(&entries.join("\n"));
        let doc = self.id();
        self.documents.insert(
            doc,
            crate::document::Document::inspection("Slate support report".into(), text),
        );
        let view = self.id();
        self.views.insert(
            view,
            crate::EditorView {
                document: doc,
                ..Default::default()
            },
        );
        let _ = self.editor_target();
        if !self.show_new_document(view) {
            return;
        }
        self.status = "Support report · read-only · select and copy to share".into();
    }
}
