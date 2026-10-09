//! Save snapshots and their confirmations outlive focus changes and dialogs.
use crate::{
    document::Document,
    fsio::{SaveErrorKind, WriteOptions},
    services::{IoJob, SaveFailure},
    App, Command, Elevation, ElevationMode,
};
use anyhow::{bail, Context, Result};
use std::path::PathBuf;

/// State that survives tab focus changes, native dialogs and save retries.
#[derive(Default)]
pub(crate) struct SaveWorkflow {
    pub pending_save: Vec<u64>,
    pub quit_after_save: bool,
    pub save_all_queue: Option<std::collections::VecDeque<u64>>,
    pub save_operations: std::collections::BTreeMap<u64, Document>,
    pub save_questions: std::collections::VecDeque<(u64, String, String)>,
    pub save_confirmation: Option<u64>,
    pub close_after_save: Option<(u64, u64)>,
    pub pending_retry: Option<(u64, Option<PathBuf>, bool)>,
    pub elevation: Option<Elevation>,
}

impl App {
    pub(crate) fn start_save(
        &mut self,
        id: u64,
        destination: Option<PathBuf>,
        overwrite: bool,
        allow_read_only: bool,
    ) -> Result<()> {
        if self.saves.pending_save.contains(&id) || self.saves.save_operations.contains_key(&id) {
            bail!("A save is already in progress for this document");
        }
        let snapshot = self.documents[&id].checkpoint();
        self.services.io.send(IoJob::Save(
            id,
            snapshot.checkpoint(),
            destination,
            overwrite,
            WriteOptions {
                allow_read_only,
                backup: self.preferences.backup,
            },
        ))?;
        self.saves.save_operations.insert(id, snapshot);
        self.saves.pending_save.push(id);
        self.status = "Saving…".into();
        Ok(())
    }

    pub(crate) fn save_all(&mut self, quit: bool) -> Result<()> {
        if self.saves.save_all_queue.is_none() {
            self.saves.save_all_queue = Some(
                self.documents
                    .iter()
                    .filter(|(_, d)| d.dirty())
                    .map(|(id, _)| *id)
                    .collect(),
            );
        }
        self.saves.quit_after_save |= quit;
        self.continue_save_all()
    }

    fn continue_save_all(&mut self) -> Result<()> {
        if self.prompt.is_some() || self.pending_path_dialog.is_some() {
            return Ok(());
        }
        loop {
            let Some(queue) = self.saves.save_all_queue.as_mut() else {
                return Ok(());
            };
            let Some(&id) = queue.front() else {
                self.saves.save_all_queue = None;
                if self.saves.quit_after_save && self.saves.pending_save.is_empty() {
                    self.saves.quit_after_save = false;
                    self.execute(Command::Quit { force: false })?;
                }
                return Ok(());
            };
            if self.saves.save_operations.contains_key(&id) || self.formatting.contains_key(&id) {
                return Ok(());
            }
            let Some(doc) = self.documents.get(&id).filter(|d| d.dirty()) else {
                queue.pop_front();
                continue;
            };
            let untitled = doc.path.is_none();
            self.reveal_document(id);
            if untitled {
                self.execute(Command::Prompt {
                    kind: "save-as".into(),
                })?;
            } else {
                if self.preferences_for_document(id).format_on_save
                    && self.format_document(id, true)?
                {
                    return Ok(());
                }
                self.start_save(id, None, false, false)?;
            }
            return Ok(());
        }
    }

    pub(crate) fn save_failed(&mut self, id: u64, failure: SaveFailure) {
        if let Some(snapshot) = failure.snapshot {
            self.saves.save_operations.insert(id, *snapshot);
        } else {
            self.saves.save_operations.remove(&id);
            self.cancel_save_workflow();
            self.status = format!("Save failed: {}", failure.message);
            return;
        }
        let kind = match failure.kind {
            SaveErrorKind::ReadOnly => "save-read-only",
            SaveErrorKind::PermissionDenied if self.elevation_mode != ElevationMode::None => {
                "save-elevated"
            }
            _ => {
                self.saves.save_operations.remove(&id);
                self.cancel_save_workflow();
                self.status = format!("Save failed: {}", failure.message);
                return;
            }
        };
        self.saves
            .save_questions
            .push_back((id, kind.into(), failure.message));
        self.offer_save_question();
    }

    pub(crate) fn offer_save_question(&mut self) {
        if self.prompt.is_some()
            || self.pending_path_dialog.is_some()
            || self.saves.save_confirmation.is_some()
        {
            return;
        }
        if let Some((id, kind, message)) = self.saves.save_questions.pop_front() {
            if !self.documents.contains_key(&id) {
                self.saves.save_operations.remove(&id);
                return;
            }
            self.reveal_document(id);
            self.saves.save_confirmation = Some(id);
            self.prompt = Some(crate::search::Prompt {
                kind,
                input: self.documents[&id].title(),
                replacement: String::new(),
                field: 0,
                case_sensitive: false,
                whole_word: false,
            });
            self.status = message;
        }
    }

    pub(crate) fn retry_save(&mut self, kind: &str) -> Result<()> {
        let id = self
            .saves
            .save_confirmation
            .take()
            .context("Nothing to save")?;
        let snapshot = self
            .saves
            .save_operations
            .get(&id)
            .context("Missing save operation")?
            .checkpoint();
        let submitted = (|| -> Result<()> {
            if kind == "save-read-only" {
                self.services.io.send(IoJob::Save(
                    id,
                    snapshot,
                    None,
                    false,
                    WriteOptions {
                        allow_read_only: true,
                        backup: self.preferences.backup,
                    },
                ))?;
            } else {
                let path = snapshot.path.clone().context("Missing save destination")?;
                match self.elevation_mode {
                    ElevationMode::Terminal => {
                        self.saves.elevation = Some(Elevation {
                            baseline: snapshot.disk.clone(),
                            document: id,
                            path,
                            bytes: snapshot.encoded()?,
                        });
                    }
                    ElevationMode::Background(program) => self
                        .services
                        .io
                        .send(IoJob::SaveElevated(id, snapshot, program.into()))?,
                    ElevationMode::None => bail!("No privilege helper is available"),
                }
            }
            Ok(())
        })();
        if let Err(error) = submitted {
            self.saves.save_operations.remove(&id);
            self.cancel_save_workflow();
            return Err(error);
        }
        self.saves.pending_save.push(id);
        self.status = "Saving…".into();
        Ok(())
    }

    pub(crate) fn cancel_save_workflow(&mut self) {
        self.saves.save_all_queue = None;
        self.saves.quit_after_save = false;
        self.saves.close_after_save = None;
        if let Some(id) = self.saves.save_confirmation.take() {
            self.saves.save_operations.remove(&id);
        }
    }

    pub(crate) fn saved_reply(&mut self, id: u64, result: Result<Document, SaveFailure>) {
        self.revision += 1;
        self.saves.pending_save.retain(|doc| *doc != id);
        match result {
            Ok(saved) => {
                self.saves.save_operations.remove(&id);
                if let Some(path) = saved.path.clone() {
                    self.remember_recent(&path);
                }
                if let Some(doc) = self.documents.get_mut(&id) {
                    doc.accept_save(saved);
                    self.status = if doc.dirty() {
                        "Saved snapshot; newer edits remain unsaved"
                    } else {
                        "Saved"
                    }
                    .into();
                    self.recovery.dirty = true;
                }
                self.lsp_saved(id);
                self.refresh();
                if let Some((pane, view)) = self.saves.close_after_save {
                    if self.views.get(&view).is_some_and(|v| v.document == id) {
                        self.saves.close_after_save = None;
                        if self.documents.get(&id).is_some_and(|d| !d.dirty()) {
                            let _ = self.close_tab(pane, view, false);
                        }
                    }
                }
                if let Some(queue) = &mut self.saves.save_all_queue {
                    if queue.front() == Some(&id) {
                        queue.pop_front();
                    }
                }
                if let Err(e) = self.continue_save_all() {
                    self.cancel_save_workflow();
                    self.status = format!("Save failed: {e:#}");
                }
            }
            Err(failure) => self.save_failed(id, failure),
        }
        self.offer_save_question();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn app(root: &std::path::Path) -> App {
        App::launch(
            &crate::cli::Launch {
                directory: Some(root.into()),
                recover: false,
                ..Default::default()
            },
            None,
        )
        .unwrap()
    }
    #[test]
    fn elevated_save_as_keeps_destination_baseline_and_snapshot() {
        let _guard = crate::paths::TEST_ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
        std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
        let original = dir.path().join("source.txt");
        let target = dir.path().join("copy.txt");
        std::fs::write(&original, "original").unwrap();
        let mut a = app(dir.path());
        let id = *a.documents.keys().next().unwrap();
        a.documents.insert(id, Document::open(&original).unwrap());
        a.documents
            .get_mut(&id)
            .unwrap()
            .replace(0, 0, "saved", 4)
            .unwrap();
        let mut snapshot = a.documents[&id].checkpoint();
        snapshot.prepare_save(Some(&target), false).unwrap();
        a.elevation_mode = ElevationMode::Terminal;
        a.saved_reply(
            id,
            Err(SaveFailure {
                kind: SaveErrorKind::PermissionDenied,
                message: "fixture".into(),
                snapshot: Some(Box::new(snapshot)),
            }),
        );
        a.documents
            .get_mut(&id)
            .unwrap()
            .replace(0, 0, "new", 4)
            .unwrap();
        a.dispatch(Command::SubmitPrompt { all: false });
        let request = a.take_elevation().unwrap();
        assert_eq!(request.path, target);
        assert!(request.baseline.is_none());
        assert_eq!(request.bytes, b"savedoriginal");
        std::fs::write(&request.path, &request.bytes).unwrap();
        a.finish_elevation(request, Ok(()));
        assert_eq!(std::fs::read_to_string(original).unwrap(), "original");
        assert_eq!(a.documents[&id].path.as_ref(), Some(&target));
        assert!(a.documents[&id].dirty());
    }
    #[test]
    fn simultaneous_save_failures_keep_their_own_confirmation_targets() {
        let _guard = crate::paths::TEST_ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
        std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
        let mut a = app(dir.path());
        let first = *a.documents.keys().next().unwrap();
        let second = a.id();
        a.documents.insert(second, Document::scratch());
        for id in [first, second] {
            let mut snapshot = a.documents[&id].checkpoint();
            snapshot
                .prepare_save(Some(&dir.path().join(format!("{id}.txt"))), false)
                .unwrap();
            a.saved_reply(
                id,
                Err(SaveFailure {
                    kind: SaveErrorKind::ReadOnly,
                    message: "fixture".into(),
                    snapshot: Some(Box::new(snapshot)),
                }),
            );
        }
        assert_eq!(a.saves.save_confirmation, Some(first));
        a.dispatch(Command::DismissPrompt);
        a.offer_save_question();
        assert_eq!(a.saves.save_confirmation, Some(second));
        assert_eq!(
            a.saves.save_operations[&second].path,
            Some(dir.path().join(format!("{second}.txt")))
        );
    }
}
