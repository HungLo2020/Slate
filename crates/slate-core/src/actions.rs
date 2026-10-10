//! Named editor actions, cursor movement, soft-wrap navigation and save
//! orchestration. Both frontends reach these through the shared catalog.
use crate::{
    document::{previous, Document},
    fsio::{SaveErrorKind, WriteOptions},
    search::{Prompt, Search},
    services::{IoJob, Job, SaveFailure},
    text_format::{self, LineEnding},
    wrap, App, Command, EditorView, Elevation,
};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use unicode_segmentation::UnicodeSegmentation;

/// The start of the previous word (`forward = false`) or the end of the next.
pub(crate) fn word_boundary(text: &str, cursor: usize, forward: bool) -> usize {
    let word = |s: &str| s.chars().any(|c| c.is_alphanumeric() || c == '_');
    if forward {
        let mut position = cursor;
        let mut seen_word = false;
        for (i, w) in text[cursor..].split_word_bound_indices() {
            if word(w) {
                seen_word = true;
            } else if seen_word || w.contains('\n') {
                return if seen_word {
                    cursor + i
                } else {
                    cursor + i + w.len()
                };
            }
            position = cursor + i + w.len();
        }
        position
    } else {
        let mut position = cursor;
        let mut seen_word = false;
        for (i, w) in text[..cursor].split_word_bound_indices().rev() {
            if word(w) {
                seen_word = true;
            } else if seen_word || w.contains('\n') {
                return if seen_word { position } else { i };
            }
            position = i;
        }
        position
    }
}

/// Word movement within a document: the cursor's line is examined, and a
/// line break is a boundary of its own.
pub(crate) fn document_word_boundary(d: &Document, cursor: usize, forward: bool) -> usize {
    let row = d.line_of(cursor);
    let (start, end) = d.line_range(row);
    if forward && cursor >= end {
        return d.next_boundary(cursor);
    }
    if !forward && cursor <= start {
        return d.previous_boundary(cursor);
    }
    let line = d.line_text(row);
    start + word_boundary(&line, cursor - start, forward)
}

pub(crate) fn prompt(kind: &str, input: String) -> Prompt {
    Prompt {
        kind: kind.into(),
        input,
        replacement: String::new(),
        field: 0,
        case_sensitive: false,
        whole_word: false,
    }
}

impl App {
    fn editor_view(&self) -> Result<(u64, EditorView)> {
        let id = self.active_editor().context("Focus an editor first")?;
        Ok((id, self.views[&id].clone()))
    }
    pub(crate) fn gutter_width(&self, doc: u64, cols: u16) -> usize {
        if !self.preferences.line_numbers {
            // Without line numbers, a narrow margin still shows breakpoints,
            // the paused line, problems and folds when there are any.
            let (breakpoints, paused) = self.debug_marks(doc);
            let marked = !breakpoints.is_empty()
                || paused.is_some()
                || !self.diagnostic_lines(doc).is_empty()
                || self
                    .views
                    .values()
                    .any(|v| v.document == doc && !v.folds.is_empty());
            return if marked { 2.min(cols as usize) } else { 0 };
        }
        let digits = self.documents[&doc].line_count().to_string().len().max(4);
        (digits + 2).min(cols as usize)
    }
    pub(crate) fn text_width(&self, id: u64) -> usize {
        let v = &self.views[&id];
        (v.cols.max(1) as usize)
            .saturating_sub(self.gutter_width(v.document, v.cols.max(1)))
            .max(1)
    }
    /// Visual rows of a logical line: one unbroken row unless soft wrap is on.
    pub(crate) fn line_rows(
        &self,
        doc: u64,
        line: usize,
        width: usize,
    ) -> std::sync::Arc<Vec<wrap::Row>> {
        let d = &self.documents[&doc];
        let (start, end) = d.line_range(line);
        let options = self.preferences_for_document(doc);
        if options.soft_wrap {
            self.wrap_cache
                .lock()
                .unwrap()
                .rows(doc, d, line, width, options.tab_width)
        } else {
            std::sync::Arc::new(vec![wrap::Row {
                start: 0,
                end: end - start,
                col: 0,
            }])
        }
    }
    /// Visual position of a byte offset: (line, row within line, column within row).
    pub(crate) fn visual_position(&self, id: u64, offset: usize) -> (usize, usize, usize) {
        let v = &self.views[&id];
        let d = &self.documents[&v.document];
        let line = d.line_of(offset);
        let (start, _) = d.line_range(line);
        let rows = self.line_rows(v.document, line, self.text_width(id));
        let row = wrap::row_of(&rows, offset - start);
        let (_, col) = d.line_col(offset, self.preferences_for_document(v.document).tab_width);
        (line, row, col - rows[row].col)
    }
    /// The byte offset shown at a column of a visual row.
    pub(crate) fn offset_in_row(
        &self,
        doc: u64,
        line: usize,
        row: wrap::Row,
        col: usize,
        last: bool,
    ) -> usize {
        let d = &self.documents[&doc];
        let (start, _) = d.line_range(line);
        let tab = self.preferences_for_document(doc).tab_width;
        let text = d.slice(start + row.start, start + row.end);
        let text = text.as_ref();
        let target = row.col + col;
        let mut x = row.col;
        for (i, g) in text.grapheme_indices(true) {
            let w = crate::document::grapheme_width(g, x, tab);
            if x + w > target {
                return start + row.start + i;
            }
            x += w;
        }
        // Past the end of a wrapped row, stay on that row.
        if !last && row.end > row.start {
            return start + previous(text, text.len()) + row.start;
        }
        start + row.end
    }
    /// Move `count` visual rows from a position, keeping the display column.
    pub(crate) fn vertical(&self, id: u64, cursor: usize, count: isize, goal: usize) -> usize {
        let v = &self.views[&id];
        let doc = v.document;
        let width = self.text_width(id);
        let (mut line, mut row, _) = self.visual_position(id, cursor);
        let mut rows = self.line_rows(doc, line, width);
        for _ in 0..count.unsigned_abs() {
            if count < 0 {
                if row > 0 {
                    row -= 1;
                } else if let Some(previous) = self.previous_shown_line(id, line) {
                    line = previous;
                    rows = self.line_rows(doc, line, width);
                    row = rows.len() - 1;
                } else {
                    return 0;
                }
            } else if row + 1 < rows.len() {
                row += 1;
            } else if let Some(next) = self.next_shown_line(id, line) {
                line = next;
                rows = self.line_rows(doc, line, width);
                row = 0;
            } else {
                return self.documents[&doc].len();
            }
        }
        // `goal` is relative to the row start when wrapping, else to the line.
        self.offset_in_row(doc, line, rows[row], goal, row + 1 == rows.len())
    }
    pub(crate) fn move_cursor(&mut self, id: u64, name: &str, extend: bool) -> Result<()> {
        let v = self.views[&id].clone();
        let doc = v.document;
        let d = &self.documents[&doc];
        let (_, _, visual_col) = self.visual_position(id, v.cursor);
        let goal = v.goal.unwrap_or(visual_col);
        let page = v.rows.max(2) as isize - 1;
        let mut keep_goal = false;
        let cursor = match name {
            "move-left" => d.previous_boundary(v.cursor),
            "move-right" => d.next_boundary(v.cursor),
            "word-left" => document_word_boundary(d, v.cursor, false),
            "word-right" => document_word_boundary(d, v.cursor, true),
            "move-up" | "move-down" | "page-up" | "page-down" => {
                keep_goal = true;
                let count = match name {
                    "move-up" => -1,
                    "move-down" => 1,
                    "page-up" => -page,
                    _ => page,
                };
                self.vertical(id, v.cursor, count, goal)
            }
            "line-start" => {
                // Toggle between the first non-blank character and column zero.
                let (start, end) = d.line_range(d.line_of(v.cursor));
                let indent = start
                    + d.slice(start, end)
                        .bytes()
                        .take_while(|b| *b == b' ' || *b == b'\t')
                        .count();
                if v.cursor == indent {
                    start
                } else {
                    indent
                }
            }
            "line-end" => d.line_range(d.line_of(v.cursor)).1,
            "document-start" => 0,
            "document-end" => d.len(),
            _ => bail!("Unknown movement: {name}"),
        };
        let view = self.views.get_mut(&id).unwrap();
        if extend || view.mark {
            view.anchor.get_or_insert(view.cursor);
        } else {
            view.anchor = None;
        }
        view.cursor = cursor;
        view.goal = keep_goal.then_some(goal);
        view.manual_scroll = false;
        // Leaving a snippet's fields ends it, so Tab indents again.
        let inside = view
            .current_stop
            .iter()
            .chain(view.stops.iter().flatten())
            .any(|(a, b)| *a <= cursor && cursor <= *b);
        if !inside {
            view.stops.clear();
            view.visited.clear();
            view.current_stop.clear();
        }
        Ok(())
    }
    /// Scroll so the cursor is visible, in visual rows when wrapping.
    pub(crate) fn ensure_visible(&mut self, id: u64) {
        // A caret never sits inside a folded region.
        let cursor_line = {
            let v = &self.views[&id];
            self.documents[&v.document].line_of(v.cursor)
        };
        self.reveal_line(id, cursor_line);
        let v = self.views[&id].clone();
        let rows = v.rows.max(1) as usize;
        let lines = self.documents[&v.document].line_count();
        if !self.preferences_for_document(v.document).soft_wrap {
            let mut top = self.shown_at_or_after(id, v.top.min(lines.saturating_sub(1)));
            if !v.manual_scroll {
                let row = cursor_line;
                if row < top {
                    top = row;
                } else if self.shown_between(id, top, row + 1) > rows {
                    // Walk back so the caret sits on the last visible row.
                    top = row;
                    for _ in 1..rows {
                        match self.previous_shown_line(id, top) {
                            Some(previous) => top = previous,
                            None => break,
                        }
                    }
                }
            }
            let view = self.views.get_mut(&id).unwrap();
            view.top_row = 0;
            view.top = top.min(lines.saturating_sub(1));
            return;
        }
        let width = self.text_width(id);
        let mut top = (v.top.min(lines.saturating_sub(1)), v.top_row);
        let top_rows = self.line_rows(v.document, top.0, width).len();
        top.1 = top.1.min(top_rows - 1);
        if !v.manual_scroll {
            let (line, row, _) = self.visual_position(id, v.cursor);
            if (line, row) < top {
                top = (line, row);
            } else {
                // Count visual rows from the top to the cursor; stop early.
                let mut distance = 0;
                let mut position = top;
                while position < (line, row) && distance < rows {
                    let count = self.line_rows(v.document, position.0, width).len();
                    if position.1 + 1 < count {
                        position.1 += 1;
                    } else {
                        match self.next_shown_line(id, position.0) {
                            Some(next) => position = (next, 0),
                            None => break,
                        }
                    }
                    distance += 1;
                }
                if distance >= rows {
                    // Walk back from the cursor so it sits on the last visible row.
                    let mut position = (line, row);
                    for _ in 0..rows - 1 {
                        if position.1 > 0 {
                            position.1 -= 1;
                        } else if let Some(previous) = self.previous_shown_line(id, position.0) {
                            position.0 = previous;
                            position.1 = self.line_rows(v.document, position.0, width).len() - 1;
                        }
                    }
                    top = position;
                }
            }
        }
        let view = self.views.get_mut(&id).unwrap();
        view.top = top.0;
        view.top_row = top.1;
    }
    /// The visible rows from the view's top: (line, row index, row).
    pub(crate) fn visible_rows(&self, id: u64, count: usize) -> Vec<(usize, usize, wrap::Row)> {
        let v = &self.views[&id];
        let width = self.text_width(id);
        let lines = self.documents[&v.document].line_count();
        let mut result = Vec::with_capacity(count);
        let first = v.top.min(lines.saturating_sub(1));
        let mut line = self.shown_at_or_after(id, first);
        let mut skip = if line == first { v.top_row } else { 0 };
        while result.len() < count && line < lines {
            let rows = self.line_rows(v.document, line, width);
            let last = rows.len() - 1;
            for (index, row) in rows.iter().copied().enumerate().skip(skip.min(last)) {
                if result.len() == count {
                    break;
                }
                result.push((line, index, row));
            }
            skip = 0;
            match self.next_shown_line(id, line) {
                Some(next) => line = next,
                None => break,
            }
        }
        result
    }
    /// The byte offset under a click in the text area of an editor view.
    pub(crate) fn click_offset(&self, id: u64, row: usize, col: usize) -> usize {
        let v = &self.views[&id];
        let gutter = self.gutter_width(v.document, v.cols.max(1));
        let col = col.saturating_sub(gutter);
        let rows = self.visible_rows(id, row + 1);
        match rows.get(row) {
            Some((line, index, r)) => {
                let left = if self.preferences_for_document(v.document).soft_wrap {
                    0
                } else {
                    v.left
                };
                let last =
                    *index + 1 == self.line_rows(v.document, *line, self.text_width(id)).len();
                self.offset_in_row(v.document, *line, *r, col + left, last)
            }
            None => self.documents[&v.document].len(),
        }
    }
    /// Scroll an editor by visual rows without moving the cursor.
    pub(crate) fn scroll_view(&mut self, id: u64, delta: i32) {
        let v = self.views[&id].clone();
        self.views.get_mut(&id).unwrap().manual_scroll = true;
        let width = self.text_width(id);
        let wrapping = self.preferences_for_document(v.document).soft_wrap;
        // Steps go by shown rows: a folded region is one step, not many.
        let (mut line, mut row) = (
            self.shown_at_or_after(id, v.top),
            if wrapping { v.top_row } else { 0 },
        );
        for _ in 0..delta.unsigned_abs() {
            if delta > 0 {
                if wrapping && row > 0 {
                    row -= 1;
                } else if let Some(previous) = self.previous_shown_line(id, line) {
                    line = previous;
                    row = if wrapping {
                        self.line_rows(v.document, line, width).len() - 1
                    } else {
                        0
                    };
                }
            } else if wrapping && row + 1 < self.line_rows(v.document, line, width).len() {
                row += 1;
            } else if let Some(next) = self.next_shown_line(id, line) {
                line = next;
                row = 0;
            }
        }
        let view = self.views.get_mut(&id).unwrap();
        view.top = line;
        view.top_row = row;
    }

    // ----- Saving -----

    /// The user confirmed a pending prompt answer.
    pub(crate) fn confirm_pending(&mut self, kind: &str) -> Result<()> {
        match kind {
            "save-read-only" | "save-elevated" => self.retry_save(kind),
            "reload-changed" => {
                let doc = self
                    .saves
                    .pending_retry
                    .take()
                    .context("Nothing to reload")?
                    .0;
                self.request_reload(doc, None)
            }
            _ => Ok(()),
        }
    }
    /// Actions the frontend must perform itself (new window, print).
    pub fn take_frontend_requests(&mut self) -> Vec<String> {
        std::mem::take(&mut self.frontend_requests)
    }
    /// A privileged save the terminal frontend should perform now.
    pub fn take_elevation(&mut self) -> Option<Elevation> {
        self.saves.elevation.take()
    }
    pub fn finish_elevation(&mut self, request: Elevation, result: Result<()>) {
        let id = request.document;
        let saved = self.saves.save_operations.remove(&id);
        let reply = match result {
            Ok(()) => saved
                .map(|mut doc| {
                    doc.mark_saved_bytes(&request.bytes);
                    Ok(doc)
                })
                .unwrap_or_else(|| {
                    Err(SaveFailure {
                        kind: SaveErrorKind::Other,
                        message: "Save operation was cancelled".into(),
                        snapshot: None,
                    })
                }),
            Err(e) => Err(SaveFailure {
                kind: SaveErrorKind::Other,
                message: format!("{e:#}"),
                snapshot: saved.map(Box::new),
            }),
        };
        let success = reply.is_ok();
        self.saved_reply(id, reply);
        if success && self.status == "Saved" {
            self.status = "Saved with sudo".into();
        }
    }
    pub(crate) fn request_reload(&mut self, doc: u64, encoding: Option<String>) -> Result<()> {
        let path = self.documents[&doc]
            .path
            .clone()
            .context("This document has no file")?;
        self.services
            .io
            .send(IoJob::Reload(
                doc,
                self.documents[&doc].content_version(),
                path,
                encoding,
            ))
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        self.status = "Reloading…".into();
        Ok(())
    }
    pub(crate) fn finish_reload(&mut self, doc: u64, mut fresh: Document) {
        let Some(old) = self.documents.get(&doc) else {
            return;
        };
        fresh.read_only = old.read_only;
        // Replacing a document must advance its render/highlight generation,
        // even when neither the old nor the new file has been edited.
        fresh.generation = old.generation.wrapping_add(1);
        let notice = fresh.notice.take();
        self.status = format!(
            "Reloaded {}{}",
            fresh.title(),
            notice.map(|n| format!(" · {n}")).unwrap_or_default()
        );
        self.documents.insert(doc, fresh);
        self.highlights.remove(&doc);
        self.wrap_cache.lock().unwrap().forget(doc);
        self.clamp_views(doc);
    }
    /// Write `NAME.save` copies of unsaved buffers, as nano does when it is
    /// killed by a signal. Returns the files written. A quarantined session
    /// refreshes its existing copies instead of adding more.
    pub fn emergency_save(&mut self) -> Vec<PathBuf> {
        if self.recovery.frozen {
            self.write_quarantine_copies();
            return self
                .documents
                .iter()
                .filter(|(_, doc)| doc.dirty())
                .filter_map(|(id, _)| Some(self.recovery.copies.get(id)?.0.clone()))
                .collect();
        }
        let mut written = vec![];
        for (id, doc) in &self.documents {
            if !doc.dirty() {
                continue;
            }
            let Ok(bytes) = doc.encoded() else { continue };
            let candidate = save_copy_target(&self.root, *id, doc);
            if crate::fsio::write_file(&candidate, &bytes, None, WriteOptions::default()).is_ok() {
                written.push(candidate);
            }
        }
        written
    }

    // ----- Editing actions -----

    fn current_line(&self, id: u64) -> (usize, usize) {
        let v = &self.views[&id];
        let d = &self.documents[&v.document];
        let row = d.line_of(v.cursor);
        let start = d.line_offset(row);
        let end = if row + 1 < d.line_count() {
            d.line_offset(row + 1)
        } else {
            d.len()
        };
        (start, end)
    }
    fn cut_line(&mut self, copy_only: bool) -> Result<()> {
        let (id, v) = self.editor_view()?;
        let doc = v.document;
        let (start, end) = match v.anchor {
            Some(a) if a != v.cursor => (a.min(v.cursor), a.max(v.cursor)),
            _ => self.current_line(id),
        };
        let text = self.documents[&doc].slice(start, end).to_string();
        let chained = !copy_only
            && v.anchor.is_none()
            && self.last_cut.is_some_and(|(view, generation, cursor)| {
                view == id && generation == self.documents[&doc].generation && cursor == v.cursor
            });
        if chained {
            self.clipboard.push_str(&text);
        } else {
            self.clipboard = text;
        }
        if copy_only {
            let view = self.views.get_mut(&id).unwrap();
            view.anchor = None;
            view.mark = false;
            self.status = "Copied text".into();
            return Ok(());
        }
        if start == end {
            bail!("Nothing to cut");
        }
        self.edit_range(id, start, end, "", v.cursor)?;
        self.last_cut = Some((id, self.documents[&doc].generation, self.views[&id].cursor));
        self.status = format!(
            "Cut {}",
            crate::counted(self.clipboard.matches('\n').count().max(1), "line", "lines")
        );
        Ok(())
    }
    /// Reflow the paragraph around the cursor to the wrap column.
    fn justify(&mut self) -> Result<()> {
        let (id, v) = self.editor_view()?;
        let d = &self.documents[&v.document];
        let blank = |row: usize| {
            let (s, e) = d.line_range(row);
            d.slice(s, e).trim().is_empty()
        };
        let (first, last) = match v.anchor {
            Some(a) if a != v.cursor => (d.line_of(a.min(v.cursor)), d.line_of(a.max(v.cursor))),
            _ => {
                let row = d.line_of(v.cursor);
                if blank(row) {
                    bail!("No paragraph at the cursor");
                }
                let mut first = row;
                while first > 0 && !blank(first - 1) {
                    first -= 1;
                }
                let mut last = row;
                while last + 1 < d.line_count() && !blank(last + 1) {
                    last += 1;
                }
                (first, last)
            }
        };
        let start = d.line_range(first).0;
        let end = d.line_range(last).1;
        let paragraph = &d.slice(start, end);
        let indent: String = paragraph
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        let width = self.preferences.wrap_column;
        let tab = self.preferences.indent_width;
        let indent_width = crate::document::display_width_with_tabs(&indent, tab);
        let mut lines = vec![];
        let mut line = indent.clone();
        let mut length = indent_width;
        for word in paragraph.split_whitespace() {
            let w = crate::document::display_width_with_tabs(word, tab);
            if length > indent_width && length + 1 + w > width {
                lines.push(std::mem::replace(&mut line, indent.clone()));
                length = indent_width;
            }
            if length > indent_width {
                line.push(' ');
                length += 1;
            }
            line.push_str(word);
            length += w;
        }
        lines.push(line);
        let value = lines.join("\n");
        self.edit_range(id, start, end, &value, v.cursor)?;
        let view = self.views.get_mut(&id).unwrap();
        view.cursor = start + value.len();
        self.status = format!("Justified {}", crate::counted(lines.len(), "line", "lines"));
        Ok(())
    }
    /// Break the current line at the last space before the wrap column.
    pub(crate) fn hard_wrap(&mut self, id: u64) -> Result<()> {
        let v = self.views[&id].clone();
        let d = &self.documents[&v.document];
        let row = d.line_of(v.cursor);
        let (start, end) = d.line_range(row);
        let line = &d.slice(start, end);
        let tab = self.preferences.indent_width;
        if crate::document::display_width_with_tabs(line, tab) <= self.preferences.wrap_column {
            return Ok(());
        }
        let limit = start + crate::document::column_offset(line, self.preferences.wrap_column, tab);
        let indent_len = line.len() - line.trim_start_matches([' ', '\t']).len();
        let Some(space) = d
            .slice(start + indent_len, limit.max(start + indent_len))
            .rfind(' ')
            .map(|i| start + indent_len + i)
        else {
            return Ok(());
        };
        let indent = line[..indent_len].to_string();
        let replacement = format!("\n{indent}");
        self.edit_range(id, space, space + 1, &replacement, v.cursor)?;
        let view = self.views.get_mut(&id).unwrap();
        view.cursor = if v.cursor > space {
            v.cursor + replacement.len() - 1
        } else {
            v.cursor
        };
        Ok(())
    }
    fn location_report(&self) -> Result<String> {
        let (_, v) = self.editor_view()?;
        let d = &self.documents[&v.document];
        let lines = d.line_count();
        let row = d.line_of(v.cursor);
        let (start, end) = d.line_range(row);
        let col = d.slice(start, v.cursor).chars().count() + 1;
        let cols = d.slice(start, end).chars().count() + 1;
        let chars = d.rope().len_chars();
        let char = d.rope().byte_to_char(v.cursor);
        let pct = |a: usize, b: usize| a * 100 / b.max(1);
        Ok(format!(
            "line {}/{lines} ({}%), col {col}/{cols} ({}%), char {char}/{chars} ({}%)",
            row + 1,
            pct(row + 1, lines),
            pct(col, cols),
            pct(char, chars)
        ))
    }
    fn help_text(&self) -> String {
        let p = &self.preferences;
        let mut out = format!(
            "Slate help · keymap: {}\n\nF1 or Ctrl+Shift+P opens the command palette, which lists every\naction with its shortcut. Type ':' in the palette to run a raw command.\n\n",
            p.keymap
        );
        let catalog = self.command_catalog("");
        for (title, map) in [
            ("Global keys", &p.global_keys),
            (
                "Editor keys (also the file and Git panes for finding and running)",
                &p.editor_keys,
            ),
            ("Terminal keys", &p.terminal_keys),
            ("While debugging", &p.debug_keys),
        ] {
            out.push_str(&format!("{title}\n"));
            for (key, action) in map {
                let name = catalog
                    .iter()
                    .find(|c| c.id == *action)
                    .map(|c| c.name.as_str())
                    .unwrap_or(action);
                out.push_str(&format!(
                    "  {:<18} {name}\n",
                    crate::commands::display_chord(key)
                ));
            }
            out.push('\n');
        }
        out.push_str(
            "Other keys\n  Shift+arrows       Select\n  Ctrl+arrows        Move by word\n  Ctrl+Backspace     Delete previous word\n  Tab / Shift+Tab    Indent / outdent a selection; next / previous snippet field\n  Tab after a prefix Expand a snippet\n  Up/Down, Tab/Enter Choose and accept a completion; Escape closes it\n  Escape             Back to one caret; closes hover text\n  Alt+click          Add or remove a caret\n  F6                 Next pane (also leaves terminal input)\n\nSettings are stored in settings.toml; choose keymap = \"nano\" for nano bindings.\n",
        );
        out
    }
    fn insert_file(&mut self, path: &str) -> Result<()> {
        let (id, _) = self.editor_view()?;
        let path = Path::new(path.trim());
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
        let bytes =
            std::fs::read(&path).with_context(|| format!("Cannot read {}", path.display()))?;
        let decoded = text_format::decode(&bytes, None)?;
        self.edit(id, &decoded.text)?;
        self.status = format!("Inserted {}", path.display());
        Ok(())
    }
    fn spell_check(&mut self) -> Result<()> {
        let (_, v) = self.editor_view()?;
        let text = self.documents[&v.document].text();
        self.services.tx.send(Job::Spell(text))?;
        self.status = "Checking spelling…".into();
        Ok(())
    }
    pub(crate) fn spelling_result(&mut self, result: std::result::Result<Vec<String>, String>) {
        match result {
            Err(e) => self.status = format!("Spell check: {e}"),
            Ok(words) if words.is_empty() => {
                self.misspellings.clear();
                self.status = "No misspellings found".into();
            }
            Ok(words) => {
                self.status = format!(
                    "{} misspelled: {}{} · next-misspelling moves between them",
                    words.len(),
                    words.iter().take(8).cloned().collect::<Vec<_>>().join(", "),
                    if words.len() > 8 { ", …" } else { "" }
                );
                self.misspellings = words;
                let _ = self.next_misspelling(true);
            }
        }
    }
    fn next_misspelling(&mut self, keep_status: bool) -> Result<()> {
        if self.misspellings.is_empty() {
            bail!("Run spell-check first");
        }
        let (id, v) = self.editor_view()?;
        let text = self.documents[&v.document].text();
        let text = text.as_str();
        let pattern = format!(
            r"\b(?:{})\b",
            self.misspellings
                .iter()
                .map(|w| regex::escape(w))
                .collect::<Vec<_>>()
                .join("|")
        );
        let regex = regex::Regex::new(&pattern)?;
        let from = v.anchor.map(|a| a.max(v.cursor)).unwrap_or(v.cursor);
        let found = regex
            .find_at(text, from.min(text.len()))
            .or_else(|| regex.find(text))
            .context("No misspellings remain")?;
        let (start, end) = (found.start(), found.end());
        let word = found.as_str().to_string();
        let view = self.views.get_mut(&id).unwrap();
        view.anchor = Some(start);
        view.cursor = end;
        view.manual_scroll = false;
        self.search = Search::new(word.clone(), true, true);
        if !keep_status {
            self.status = format!("Misspelled: {word}");
        }
        Ok(())
    }
    fn set_format(
        &mut self,
        change: impl FnOnce(&mut text_format::TextFormat) -> Result<()>,
    ) -> Result<()> {
        let (_, v) = self.editor_view()?;
        let doc = self.documents.get_mut(&v.document).unwrap();
        let mut format = doc.format.clone();
        change(&mut format)?;
        format.mixed_line_endings = false;
        self.document_search(crate::document_search::Mode::Format(format))
    }

    /// Execute a named action. Arguments come from prompts or the command line.
    pub(crate) fn run_action(&mut self, name: &str, argument: &str) -> Result<()> {
        if ["justify", "spell-check", "next-misspelling"].contains(&name) {
            let (_, view) = self.editor_view()?;
            anyhow::ensure!(
                self.documents[&view.document].len() <= 1024 * 1024,
                "This command is limited to 1 MiB documents; search and editing remain available"
            );
        }
        match name {
            "move-left" | "move-right" | "move-up" | "move-down" | "word-left" | "word-right"
            | "line-start" | "line-end" | "page-up" | "page-down" | "document-start"
            | "document-end" => {
                let (id, _) = self.editor_view()?;
                self.move_cursor(id, name, false)?;
            }
            "mark" => {
                let (id, v) = self.editor_view()?;
                let view = self.views.get_mut(&id).unwrap();
                if v.mark {
                    view.mark = false;
                    view.anchor = None;
                    self.status = "Mark unset".into();
                } else {
                    view.mark = true;
                    view.anchor = Some(v.cursor);
                    self.status = "Mark set".into();
                }
            }
            "cut-line" => self.cut_line(false)?,
            "copy-line" => self.cut_line(true)?,
            "uncut" => {
                let (id, _) = self.editor_view()?;
                if self.clipboard.is_empty() {
                    bail!("The cut buffer is empty");
                }
                let text = self.clipboard.clone();
                self.edit(id, &text)?;
            }
            "justify" => self.justify()?,
            "word-count" => self.document_search(crate::document_search::Mode::Statistics)?,
            "location" => self.status = self.location_report()?,
            "spell-check" => self.spell_check()?,
            "next-misspelling" => self.next_misspelling(false)?,
            "help" => {
                // One Help document, refreshed (key tables may have changed).
                let label = "Help · read-only";
                let text = self.help_text();
                let existing = self
                    .documents
                    .iter()
                    .find(|(_, d)| d.label.as_deref() == Some(label))
                    .map(|(id, _)| *id);
                let doc = match existing {
                    Some(doc) => {
                        let d = self.documents.get_mut(&doc).unwrap();
                        let len = d.len();
                        d.replace_inspection(0, len, &text);
                        d.generation += 1;
                        self.clamp_views(doc);
                        doc
                    }
                    None => {
                        let doc = self.id();
                        self.documents
                            .insert(doc, Document::inspection(label.into(), text));
                        doc
                    }
                };
                self.reveal_document(doc);
            }
            "previous-tab" => self.cycle_tab(-1)?,
            "language-servers" => self.show_language_servers(),
            "insert-file" if argument.trim().is_empty() => {
                self.prompt = Some(prompt("insert-file", String::new()));
            }
            "insert-file" => self.insert_file(argument)?,
            "profile-save" | "profile-load" => self.profile_action(name, argument)?,
            "new-file"
            | "new-folder"
            | "rename-file"
            | "trash-file"
            | "toggle-folder"
            | "expand-folder"
            | "collapse-folder"
            | "collapse-all-folders"
            | "terminal-here" => self.file_action(name, argument)?,
            "save-all" => self.save_all(false)?,
            "reload" => {
                let (_, v) = self.editor_view()?;
                if self.documents[&v.document].dirty() && argument != "force" {
                    self.saves.pending_retry = Some((v.document, None, false));
                    self.prompt = Some(prompt(
                        "reload-changed",
                        self.documents[&v.document].title(),
                    ));
                    return Ok(());
                }
                self.request_reload(v.document, None)?;
            }
            "reopen-encoding" => {
                let (_, v) = self.editor_view()?;
                if argument.trim().is_empty() {
                    self.prompt = Some(prompt("reopen-encoding", String::new()));
                    return Ok(());
                }
                if self.documents[&v.document].dirty() {
                    bail!("Save or discard changes before reinterpreting the file");
                }
                let name = text_format::encoding_name(argument)?;
                self.request_reload(v.document, Some(name))?;
            }
            "set-encoding" => {
                if argument.trim().is_empty() {
                    self.prompt = Some(prompt("set-encoding", String::new()));
                    return Ok(());
                }
                let name = text_format::encoding_name(argument)?;
                self.set_format(|f| {
                    f.bom = f.bom && name.starts_with("UTF");
                    f.encoding = name;
                    Ok(())
                })?;
            }
            "set-line-ending" => {
                if argument.trim().is_empty() {
                    self.prompt = Some(prompt("set-line-ending", String::new()));
                    return Ok(());
                }
                let ending: LineEnding = argument.trim().parse()?;
                self.set_format(|f| {
                    f.line_ending = ending;
                    Ok(())
                })?;
            }
            "toggle-bom" => self.set_format(|f| {
                if !f.encoding.starts_with("UTF") {
                    bail!("Only Unicode encodings have a byte-order mark");
                }
                f.bom = !f.bom;
                Ok(())
            })?,
            "toggle-read-only" => {
                let (_, v) = self.editor_view()?;
                let doc = self.documents.get_mut(&v.document).unwrap();
                if doc.label.is_some() {
                    bail!("Inspection views are always read-only");
                }
                doc.read_only = !doc.read_only;
                doc.generation += 1;
                self.status = if doc.read_only {
                    "Read-only".into()
                } else {
                    "Editing enabled".into()
                };
            }
            "toggle-soft-wrap" => {
                let value = (!self.preferences.soft_wrap).to_string();
                self.configure("soft-wrap", &value)?;
                for view in self.views.values_mut() {
                    view.top_row = 0;
                    view.left = 0;
                }
            }
            "toggle-line-numbers" => {
                let value = (!self.preferences.line_numbers).to_string();
                self.configure("line-numbers", &value)?;
            }
            "toggle-mouse" => {
                let value = (!self.preferences.tui_mouse).to_string();
                self.configure("tui-mouse", &value)?;
            }
            "toggle-backup" => {
                let value = (!self.preferences.backup).to_string();
                self.configure("backup", &value)?;
            }
            "suspend" => self.suspend_requested = true,
            "toggle-whitespace" => {
                let value = (!self.preferences.show_whitespace).to_string();
                self.configure("show-whitespace", &value)?;
            }
            "toggle-minimap" => {
                let value = (!self.preferences.minimap).to_string();
                self.configure("minimap", &value)?;
            }
            "toggle-auto-reload" => {
                let value = (!self.preferences.auto_reload).to_string();
                self.configure("auto-reload", &value)?;
            }
            "zoom-in" | "zoom-out" | "zoom-reset" => {
                let size = match name {
                    "zoom-in" => (self.preferences.font_size + 1).min(72),
                    "zoom-out" => self.preferences.font_size.saturating_sub(1).max(6),
                    _ => 11,
                };
                self.configure("font-size", &size.to_string())?;
            }
            "open-recent" => {
                let target = argument.trim();
                if target.is_empty() {
                    self.prompt = Some(prompt(
                        "open-recent",
                        self.recent.first().cloned().unwrap_or_default(),
                    ));
                    return Ok(());
                }
                let path = target
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| self.recent.get(n.checked_sub(1)?).cloned())
                    .unwrap_or_else(|| target.to_string());
                self.execute(Command::Open { path: path.into() })?;
            }
            "open-recent-project" => self.open_recent_project(argument)?,
            "support-report" => self.support_report(),
            "search-settings" => self.search_settings(argument)?,
            "diff-side-by-side" => self.request_comparison()?,
            "stage-hunk" => self.stage_diff_hunk(false)?,
            "unstage-hunk" => self.stage_diff_hunk(true)?,
            "accept-ours" => self.resolve_conflict("ours")?,
            "accept-theirs" => self.resolve_conflict("theirs")?,
            "accept-both" => self.resolve_conflict("both")?,
            "clear-recent" => {
                self.recent.clear();
                let _ = std::fs::remove_file(crate::paths::state_dir().join("slate/recent.json"));
                self.status = "Cleared recent files".into();
            }
            "new-window" | "print" => self.frontend_requests.push(name.to_string()),
            "add-cursor-above" | "add-cursor-below" => {
                let (id, _) = self.editor_view()?;
                self.add_cursor_vertical(id, if name == "add-cursor-above" { -1 } else { 1 })?;
            }
            "add-next-occurrence" => {
                let (id, _) = self.editor_view()?;
                self.add_next_occurrence(id)?;
            }
            "select-all-occurrences" => {
                let (id, _) = self.editor_view()?;
                self.select_all_occurrences(id)?;
            }
            "go-to-bracket" => {
                let (id, v) = self.editor_view()?;
                let rope = self.documents[&v.document].rope();
                let (a, b) = crate::smart::matching_bracket(rope, v.cursor)
                    .context("No bracket at the cursor")?;
                // Jump to the other bracket of the pair.
                let target = if v.cursor == a || v.cursor == a + 1 {
                    b
                } else {
                    a
                };
                let view = self.views.get_mut(&id).unwrap();
                view.cursor = target;
                view.anchor = None;
                view.extra.clear();
            }
            "fold" | "unfold" | "toggle-fold" => {
                let (id, _) = self.editor_view()?;
                self.fold(
                    id,
                    match name {
                        "fold" => Some(true),
                        "unfold" => Some(false),
                        _ => None,
                    },
                )?;
            }
            "fold-all" | "unfold-all" => {
                let (id, _) = self.editor_view()?;
                self.fold_all(id, name == "fold-all");
            }
            "quick-open" => self.open_picker("files", argument)?,
            "go-to-symbol" => self.open_picker("symbols", argument)?,
            "search-in-files" => {
                // Start from the selection, as other editors do.
                let selected = self
                    .active_editor()
                    .and_then(|id| {
                        let v = &self.views[&id];
                        let (a, b) = (v.anchor?.min(v.cursor), v.anchor?.max(v.cursor));
                        let text = self.documents[&v.document].slice(a, b).into_owned();
                        (!text.contains('\n')).then_some(text)
                    })
                    .unwrap_or_default();
                let query = if argument.is_empty() {
                    &selected
                } else {
                    argument
                };
                self.open_picker("search", query)?
            }
            "replace-in-files" => self.begin_replace_in_files()?,
            "undo-replace-in-files" => self.undo_replace_in_files()?,
            "open-documents" => self.open_picker("documents", argument)?,
            "problems" => self.open_picker("diagnostics", argument)?,
            "signature-help" => self.lsp_signature()?,
            "select-rectangle" => {
                self.execute(crate::Command::SelectRectangle)?;
            }
            "hover" => self.lsp_hover()?,
            "go-to-definition" => self.lsp_definition()?,
            "find-references" => self.lsp_references()?,
            "rename-symbol" if argument.trim().is_empty() => {
                let (id, v) = self.editor_view()?;
                let doc = &self.documents[&v.document];
                let start = crate::actions::document_word_boundary(doc, v.cursor, false);
                let end = crate::actions::document_word_boundary(doc, start, true);
                let word = doc
                    .slice(start.min(v.cursor), end.max(v.cursor))
                    .trim()
                    .to_string();
                let _ = id;
                self.prompt = Some(prompt("rename-symbol", word));
            }
            "rename-symbol" => self.lsp_rename(argument)?,
            "code-actions" => self.lsp_code_actions()?,
            "apply-code-action" => self.apply_code_action(argument)?,
            "format-document" => {
                let (_, v) = self.editor_view()?;
                self.format_document(v.document, false)?;
            }
            "trigger-completion" => self.lsp_complete(true)?,
            "workspace-symbols" => self.open_picker("workspace-symbols", argument)?,
            "next-problem" => self.next_problem(false)?,
            "previous-problem" => self.next_problem(true)?,
            "restart-language-servers" => {
                self.lsp_restart();
                self.status = "Restarting language servers…".into();
            }
            "debug-start" | "debug-program" => self.debug_start(argument)?,
            "debug-continue" => self.debug_control("continue")?,
            "debug-step-over" => self.debug_control("next")?,
            "debug-step-into" => self.debug_control("stepIn")?,
            "debug-step-out" => self.debug_control("stepOut")?,
            "debug-pause" => self.debug_control("pause")?,
            "debug-stop" => self.debug_control("stop")?,
            "debug-evaluate" if argument.trim().is_empty() => {
                self.prompt = Some(prompt("debug-evaluate", String::new()));
            }
            "debug-evaluate" => self.debug_evaluate(argument)?,
            "toggle-breakpoint" => self.toggle_breakpoint()?,
            "clear-breakpoints" => self.clear_breakpoints()?,
            "breakpoints" => self.breakpoint_picker()?,
            "debug-call-stack" => self.frame_picker()?,
            "debug-frame" => self.select_frame(argument.trim().parse()?)?,
            "run-tool" => self.run_tool(argument)?,
            "run-task" if argument.is_empty() => self.task_picker()?,
            "run-task" => self.run_task(argument)?,
            "run-build-task" => self.run_task("")?,
            "stop-task" => self.stop_tasks()?,
            "go-back" => self.go_back(false)?,
            "go-forward" => self.go_back(true)?,
            "request-trust" => self.request_trust(),
            "trust-workspace" => self.set_workspace_trust(true)?,
            "restrict-workspace" => self.set_workspace_trust(false)?,
            _ => bail!("Unknown action: {name}"),
        }
        Ok(())
    }
}

/// A new `NAME.save` file beside the document (untitled buffers: in the
/// workspace root), never replacing an existing file.
pub(crate) fn save_copy_target(root: &Path, id: u64, doc: &Document) -> PathBuf {
    let target = match &doc.path {
        Some(path) => {
            let mut name = path.file_name().unwrap_or_default().to_os_string();
            name.push(".save");
            path.with_file_name(name)
        }
        None => root.join(format!("slate.{}.{id}.save", std::process::id())),
    };
    let mut candidate = target.clone();
    let mut n = 1;
    while candidate.exists() {
        candidate = PathBuf::from(format!("{}.{n}", target.display()));
        n += 1;
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::word_boundary;

    #[test]
    fn word_boundaries_skip_punctuation_and_stop_at_lines() {
        let text = "let foo_bar = baz(1);\nnext";
        assert_eq!(word_boundary(text, 0, true), 3);
        assert_eq!(word_boundary(text, 3, true), 11);
        assert_eq!(word_boundary(text, 11, false), 4);
        assert_eq!(word_boundary(text, 4, false), 0);
        assert_eq!(word_boundary(text, 22, false), 21);
    }
}
