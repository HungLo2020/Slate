//! Apply bounded batches of worker replies on the owning event loop.
use crate::*;

impl App {
    pub fn poll(&mut self) {
        let started = Instant::now();
        for processed in 0..128 {
            if started.elapsed() >= Duration::from_millis(4) {
                self.events.notify();
                break;
            }
            let Ok(reply) = self.services.rx.try_recv() else {
                break;
            };
            if processed == 127 {
                self.events.notify();
            }
            self.revision += 1;
            match reply {
                Reply::PreparedSave(request, result) => {
                    self.prepared_save(*request, result.map(|p| *p))
                }
                Reply::DocumentSearch(request, result) => {
                    self.document_search_done(request, result)
                }
                Reply::Checkpoint(fingerprint, result) => {
                    self.checkpoint_finished(fingerprint, result)
                }
                Reply::OpenedDirectory(token, path) => {
                    self.editor_wait_opened(
                        token,
                        Err(anyhow::anyhow!("--wait requires files, not directories")),
                    );
                    if let Some(pane) = self.pending_open.remove(&token) {
                        if self.layout.view(pane).is_some() {
                            self.focus = pane;
                        }
                        if let Err(e) = self.browse(path) {
                            self.status = format!("Browse failed: {e:#}");
                        }
                    }
                }
                Reply::Opened(token, result) => {
                    if let Some(pane) = self.pending_open.remove(&token) {
                        match result {
                            Ok(d) => {
                                let doc = self.finish_open(pane, d);
                                self.editor_wait_opened(token, Ok(doc));
                            }
                            Err(e) => {
                                self.status = format!("Open failed: {e}");
                                self.editor_wait_opened(token, Err(anyhow::anyhow!(e)));
                            }
                        }
                    }
                }
                Reply::Saved(id, result) => self.saved_reply(id, result),
                Reply::Reloaded(id, version, result) => match result {
                    Ok(d)
                        if self
                            .documents
                            .get(&id)
                            .is_some_and(|doc| doc.content_version() == version) =>
                    {
                        self.finish_reload(id, d)
                    }
                    Ok(_) => {
                        self.status =
                            "Reload cancelled: the document was edited while loading".into()
                    }
                    Err(e) => self.status = format!("Reload failed: {e}"),
                },
                Reply::Overview(overview, tab) => self.overview_ready(overview, tab),
                Reply::Highlighted(response) => self.highlighted(response),
                Reply::FileOperation(operation, result) => {
                    self.file_operation_done(operation, result)
                }
                Reply::Files(path, Ok(entries)) if path == self.browser => {
                    if self.files != entries {
                        self.files_revision += 1;
                    }
                    self.files = entries;
                    self.selected = self.selected.min(self.files.len().saturating_sub(1));
                }
                Reply::Files(_, Err(e)) => self.status = e,
                Reply::Git(state) => self.git_status(state),
                Reply::GitOperation(result) => self.git_operation_done(result),
                Reply::Index(root, files) => self.index_ready(root, files),
                Reply::Filtered(generation, rows) => self.filtered_picker(generation, rows),
                Reply::SearchHits(generation, hits) => self.search_hits(generation, hits),
                Reply::SearchDone(generation, result) => self.search_done(generation, result),
                Reply::Outline(doc, symbols) => self.symbols_ready(doc, symbols),
                Reply::Replaced(report) => self.replaced(report),
                Reply::Ide(event) => self.ide_event(event),
                Reply::Previews(generation, previews) => self.previews_ready(generation, previews),
                Reply::ToolDone(name, output, target, result) => {
                    self.tool_done(name, output, target, result)
                }
                Reply::Formatted(doc, content, result) => self.formatted(doc, content, result),
                Reply::Spelling(result) => self.spelling_result(result),
                Reply::Disk(changes) => self.disk_changes(changes),
                Reply::GitPreview(Ok(preview)) => self.show_comparison(preview),
                Reply::GitPreview(Err(error)) => self.git_operation_done(Err(error)),
                _ => {}
            }
        }
        self.offer_save_question();
        self.poll_editor_waits();
    }
}
