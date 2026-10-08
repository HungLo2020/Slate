//! Desktop-editor behaviour shared by both frontends: watching open files
//! for external changes, recent files, word/line selection, horizontal
//! scrolling and the document overview used by the GUI minimap.
use crate::{
    document::{DiskChange, Document},
    search::Prompt,
    services::IoJob,
    App,
};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use unicode_segmentation::UnicodeSegmentation;

const RECENT_LIMIT: usize = 20;

/// A compact outline of a document for the minimap: per sampled line, its
/// indentation and length in display columns.
#[derive(Clone, Serialize)]
pub struct Overview {
    pub document: u64,
    pub generation: u64,
    pub total: usize,
    pub lines: Vec<[u16; 2]>,
    pub widest: usize,
}

fn recent_path() -> PathBuf {
    crate::paths::state_dir().join("slate/recent.json")
}
pub(crate) fn load_recent() -> Vec<String> {
    std::fs::read(recent_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Vec<String>>(&bytes).ok())
        .map(|mut list| {
            list.truncate(RECENT_LIMIT);
            list
        })
        .unwrap_or_default()
}

impl App {
    /// Ask the IO worker, at most once a second, whether open files changed.
    pub(crate) fn watch_files(&mut self) {
        if self.disk_check_pending || self.disk_check_at.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.disk_check_at = Instant::now();
        let entries: Vec<_> = self
            .documents
            .iter()
            .filter(|(id, _)| !self.pending_save.contains(id))
            .filter_map(|(id, d)| {
                d.watch_entry()
                    .map(|(path, baseline, stamp, encoding)| (*id, path, baseline, stamp, encoding))
            })
            .collect();
        if entries.is_empty() {
            return;
        }
        if self.services.io.send(IoJob::CheckDisk(entries)).is_ok() {
            self.disk_check_pending = true;
        }
    }
    pub(crate) fn disk_changes(&mut self, changes: Vec<(u64, DiskChange)>) {
        self.disk_check_pending = false;
        for (id, change) in changes {
            // A save that started after the check owns the baseline now.
            if self.pending_save.contains(&id) {
                continue;
            }
            let Some(document) = self.documents.get_mut(&id) else {
                continue;
            };
            let title = document.title();
            match change {
                DiskChange::Touched(stamp) => document.stamp = stamp,
                DiskChange::Unreadable => {}
                DiskChange::Missing => {
                    if !document.unavailable() {
                        document.mark_unavailable();
                        document.stamp = None;
                        self.status =
                            format!("{title} was deleted or moved on disk; save to recreate it");
                    }
                }
                DiskChange::Changed(fresh) => {
                    let fresh = *fresh;
                    if !document.dirty() && self.preferences.auto_reload {
                        self.finish_reload(id, fresh);
                        self.status = format!("{title} changed on disk and was reloaded");
                    } else {
                        // Remember the version so the watcher does not ask again,
                        // and let the user decide.
                        document.stamp = fresh.stamp;
                        self.disk_conflicts.retain(|(doc, _)| *doc != id);
                        self.disk_conflicts.push_back((id, fresh));
                    }
                }
            }
        }
        self.offer_disk_conflict();
    }
    /// Show the next "file changed on disk" question when nothing else is asked.
    pub(crate) fn offer_disk_conflict(&mut self) {
        if self.prompt.is_some() {
            return;
        }
        while let Some((id, _)) = self.disk_conflicts.front() {
            if let Some(document) = self.documents.get(id) {
                let title = document.title();
                self.reveal_document(*id);
                self.prompt = Some(Prompt {
                    kind: "file-changed".into(),
                    input: title,
                    replacement: String::new(),
                    field: 0,
                    case_sensitive: false,
                    whole_word: false,
                });
                return;
            }
            self.disk_conflicts.pop_front();
        }
    }
    /// Answer the "file changed on disk" question: reload, or keep the
    /// editor's version (the next save replaces the disk version).
    pub(crate) fn resolve_disk_conflict(&mut self, reload: bool) {
        self.prompt = None;
        if let Some((id, fresh)) = self.disk_conflicts.pop_front() {
            if reload {
                self.finish_reload(id, fresh);
            } else if let Some(document) = self.documents.get_mut(&id) {
                document.adopt_disk_baseline(&fresh);
                self.status = format!(
                    "Kept your version of {}; saving will replace the file on disk",
                    document.title()
                );
            }
        }
        self.offer_disk_conflict();
    }
    /// Put `path` first in the recent files list and save the list.
    pub(crate) fn remember_recent(&mut self, path: &Path) {
        let path = path.to_string_lossy().into_owned();
        self.recent.retain(|p| *p != path);
        self.recent.insert(0, path);
        self.recent.truncate(RECENT_LIMIT);
        if let Ok(json) = serde_json::to_vec(&self.recent) {
            // The list is a convenience; failing to save it is not an error.
            let _ = crate::fsio::write_private(&recent_path(), &json);
        }
    }
    pub fn recent_files(&self) -> &[String] {
        &self.recent
    }
    /// Window title: the active document (with a modified marker) and folder.
    pub(crate) fn window_title(&self) -> String {
        let folder = self
            .root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.root.display().to_string());
        let document = self
            .active_editor()
            .or_else(|| {
                self.layout
                    .panes()
                    .into_iter()
                    .find_map(|p| match self.layout.view(p) {
                        Some(crate::layout::View::Editor(id)) => Some(*id),
                        _ => None,
                    })
            })
            .and_then(|id| self.views.get(&id))
            .and_then(|v| self.documents.get(&v.document));
        match document {
            Some(d) => format!("{} — {folder} — Slate", d.title()),
            None => format!("{folder} — Slate"),
        }
    }
    /// Title and text of the focused (or first) editor's document, for printing.
    pub fn active_document(&self) -> Option<(String, String)> {
        let view = self.active_editor().or_else(|| {
            self.layout
                .panes()
                .into_iter()
                .find_map(|p| match self.layout.view(p) {
                    Some(crate::layout::View::Editor(id)) => Some(*id),
                    _ => None,
                })
        })?;
        let document = self.documents.get(&self.views.get(&view)?.document)?;
        Some((document.title(), document.text()))
    }
    /// Select the word (double click) or line (triple click) at the cursor.
    pub(crate) fn select_unit(&mut self, id: u64, line: bool) {
        let v = self.views[&id].clone();
        let d = &self.documents[&v.document];
        let (start, end) = if line {
            let row = d.line_of(v.cursor);
            let start = d.line_offset(row);
            let end = if row + 1 < d.line_count() {
                d.line_offset(row + 1)
            } else {
                d.len()
            };
            (start, end)
        } else {
            let row = d.line_of(v.cursor);
            let (line_start, line_end) = d.line_range(row);
            let text = &d.slice(line_start, line_end);
            let offset = v.cursor - line_start;
            let mut range = (v.cursor, v.cursor);
            for (i, word) in text.split_word_bound_indices() {
                if offset >= i && offset < i + word.len()
                    || offset == i + word.len() && offset == text.len()
                {
                    range = (line_start + i, line_start + i + word.len());
                    break;
                }
            }
            range
        };
        let view = self.views.get_mut(&id).unwrap();
        view.anchor = Some(start);
        view.cursor = end;
        view.goal = None;
    }
    /// Scroll an editor horizontally by display columns.
    pub(crate) fn scroll_columns(&mut self, id: u64, delta: i32) {
        if self.preferences.soft_wrap {
            return;
        }
        let widest = self.overview_for(self.views[&id].document).widest;
        let view = self.views.get_mut(&id).unwrap();
        let limit = widest.saturating_sub(view.cols as usize / 2);
        view.left = (view.left as i64 + delta as i64).clamp(0, limit as i64) as usize;
        view.manual_scroll = true;
    }
    /// The (cached) minimap outline of a document.
    pub fn overview_for(&mut self, doc: u64) -> Overview {
        let generation = self.documents[&doc].generation;
        if let Some(cached) = self.overview_cache.get(&doc) {
            if cached.generation == generation {
                return cached.clone();
            }
        }
        let d = &self.documents[&doc];
        let total = d.line_count();
        let tab = self.preferences.indent_width;
        // Large documents are sampled; the minimap has a few thousand pixels at most.
        let samples = total.min(4000);
        let mut lines = Vec::with_capacity(samples);
        for index in 0..samples {
            let row = index * total / samples.max(1);
            let (start, mut end) = d.line_range(row);
            end = d.floor_boundary(end.min(start + 4096));
            let line = &d.slice(start, end);
            let trimmed = line.trim_start_matches([' ', '\t']);
            let indent =
                crate::document::display_width_with_tabs(&line[..line.len() - trimmed.len()], tab);
            let width = crate::document::display_width_with_tabs(line, tab);
            lines.push([
                indent.min(u16::MAX as usize) as u16,
                width.min(u16::MAX as usize) as u16,
            ]);
        }
        // The horizontal scroll range needs every line. A line is at most
        // `tab` columns per byte wide, so only candidates are measured.
        let mut widest = 0;
        for row in 0..total {
            let (start, end) = d.line_range(row);
            if (end - start) * tab.max(1) > widest {
                widest = widest.max(crate::document::display_width_with_tabs(
                    &d.slice(start, end),
                    tab,
                ));
            }
        }
        let overview = Overview {
            document: doc,
            generation,
            total,
            lines,
            widest,
        };
        self.overview_cache.insert(doc, overview.clone());
        overview
    }
    /// The minimap outline of the editor shown in a pane.
    pub fn pane_overview(&mut self, pane: u64) -> Option<Overview> {
        match self.layout.view(pane) {
            Some(crate::layout::View::Editor(id)) => {
                let doc = self.views.get(id)?.document;
                Some(self.overview_for(doc))
            }
            _ => None,
        }
    }
}

impl Document {
    pub(crate) fn unavailable(&self) -> bool {
        self.unavailable_flag()
    }
}

/// Blend two `#rrggbb` colours (used for whitespace markers).
pub(crate) fn blend(a: &str, b: &str, amount: f32) -> String {
    let parse = |s: &str| u32::from_str_radix(s.trim_start_matches('#'), 16).unwrap_or(0);
    let (a, b) = (parse(a), parse(b));
    let channel = |shift: u32| {
        let x = ((a >> shift) & 255) as f32;
        let y = ((b >> shift) & 255) as f32;
        (x + (y - x) * amount).round() as u32
    };
    format!("#{:02x}{:02x}{:02x}", channel(16), channel(8), channel(0))
}

#[cfg(test)]
mod tests {
    #[test]
    fn colours_blend() {
        assert_eq!(super::blend("#000000", "#ffffff", 0.5), "#808080");
        assert_eq!(super::blend("#102030", "#102030", 0.3), "#102030");
    }
}
