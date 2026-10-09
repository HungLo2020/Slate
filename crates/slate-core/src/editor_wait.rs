//! Track each editor-launch request through opening and the last visible view.
use crate::{
    instance::{OpenRequest, WaitTicket},
    services::IoJob,
    App,
};
use anyhow::Result;
use std::{collections::BTreeSet, path::PathBuf};

pub(crate) struct Waiting {
    ticket: WaitTicket,
    opening: BTreeSet<u64>,
    documents: BTreeSet<u64>,
}
impl App {
    pub(crate) fn queue_open(&mut self, path: PathBuf) -> Result<u64> {
        let path = if path.is_absolute() {
            path
        } else {
            self.root.join(path)
        };
        let token = self.id();
        self.services.io.send(IoJob::Open(token, path))?;
        self.pending_open.insert(token, self.focus);
        self.status = "Opening file…".into();
        Ok(token)
    }
    pub(crate) fn open_editor_request(&mut self, request: OpenRequest) {
        let mut opening = BTreeSet::new();
        let mut failure = None;
        for file in request.files {
            if self.quit || file.path.is_dir() {
                failure = Some("The requested file cannot be opened in this window".to_string());
                break;
            }
            if let (Some(line), true) = (file.line, file.path.exists()) {
                self.pending_positions.insert(
                    file.path.clone(),
                    (
                        line.saturating_sub(1),
                        crate::navigation::Column::Display(
                            file.column.unwrap_or(1).saturating_sub(1),
                        ),
                    ),
                );
            }
            match self.queue_open(file.path) {
                Ok(token) => {
                    opening.insert(token);
                }
                Err(error) => {
                    failure = Some(format!("Cannot open requested file: {error}"));
                    break;
                }
            }
        }
        if let Some(error) = &failure {
            self.status = format!("Open failed: {error}");
        }
        if let Some(ticket) = request.ticket {
            if let Some(error) = failure {
                ticket.finish(Some(&error));
            } else {
                self.editor_waits.push(Waiting {
                    ticket,
                    opening,
                    documents: BTreeSet::new(),
                });
            }
        }
    }
    /// Attach a launcher to documents synchronously opened at startup.
    pub fn attach_editor_wait(
        &mut self,
        ticket: WaitTicket,
        files: &[crate::cli::LaunchFile],
    ) -> Result<()> {
        let documents = (|| -> Result<BTreeSet<u64>> {
            let mut documents = BTreeSet::new();
            for file in files {
                let path = crate::document::absolute(&file.path)?;
                let matches: Vec<_> = self
                    .documents
                    .iter()
                    .filter(|(_, d)| {
                        d.path
                            .as_deref()
                            .is_some_and(|p| crate::navigation::same_file(p, &path))
                    })
                    .map(|(id, _)| *id)
                    .collect();
                anyhow::ensure!(
                    !matches.is_empty(),
                    "Requested file was not opened: {}",
                    file.path.display()
                );
                documents.extend(matches);
            }
            Ok(documents)
        })();
        match documents {
            Ok(documents) => {
                ticket.ready()?;
                self.editor_waits.push(Waiting {
                    ticket,
                    opening: BTreeSet::new(),
                    documents,
                });
                Ok(())
            }
            Err(error) => {
                ticket.finish(Some(&error.to_string()));
                Err(error)
            }
        }
    }
    pub(crate) fn editor_wait_opened(&mut self, token: u64, result: Result<u64>) {
        let mut keep = Vec::new();
        for mut waiting in std::mem::take(&mut self.editor_waits) {
            if waiting.opening.remove(&token) {
                match &result {
                    Ok(doc) => {
                        waiting.documents.insert(*doc);
                    }
                    Err(error) => {
                        waiting.ticket.finish(Some(&error.to_string()));
                        continue;
                    }
                }
            }
            keep.push(waiting);
        }
        self.editor_waits = keep;
    }
    pub(crate) fn poll_editor_waits(&mut self) {
        if self.editor_waits.is_empty() {
            return;
        }
        let visible: BTreeSet<_> = self
            .layout
            .panes()
            .into_iter()
            .flat_map(|pane| self.layout.tabs(pane).unwrap_or_default())
            .filter_map(|view| match view {
                crate::layout::View::Editor(id) => self.views.get(id).map(|v| v.document),
                _ => None,
            })
            .collect();
        let mut keep = Vec::new();
        for mut waiting in std::mem::take(&mut self.editor_waits) {
            if !waiting.ticket.connected() {
                continue;
            }
            waiting.documents.retain(|doc| visible.contains(doc));
            if waiting.opening.is_empty() && waiting.documents.is_empty() {
                waiting.ticket.finish(None);
            } else {
                keep.push(waiting);
            }
        }
        self.editor_waits = keep;
    }
    /// An orderly window close finishes loaded files; termination and failed
    /// startup/opening must not look like successful editor completion.
    pub fn finish_editor_waits(&mut self, error: Option<&str>) {
        for waiting in std::mem::take(&mut self.editor_waits) {
            waiting.ticket.finish(error.or_else(|| {
                (!waiting.opening.is_empty())
                    .then_some("GUI closed before the requested files opened")
            }));
        }
    }
}
