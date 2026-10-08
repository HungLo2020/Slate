//! Indentation-based code folding. A view remembers folded regions by the
//! byte offset of their header line, so edits elsewhere move them along;
//! the hidden line ranges are recomputed (and cached) from the text.
use crate::{smart, App};
use anyhow::{bail, Result};
use std::sync::Arc;

pub(crate) type Hidden = Arc<Vec<(usize, usize)>>;

impl App {
    /// Hidden line ranges of a view, inclusive, sorted and disjoint.
    pub(crate) fn hidden_lines(&self, id: u64) -> Hidden {
        let v = &self.views[&id];
        if v.folds.is_empty() {
            return Hidden::default();
        }
        let doc = &self.documents[&v.document];
        let key = (
            doc.generation,
            v.folds.clone(),
            self.preferences_for_document(v.document).indent_width,
        );
        let mut cache = self.fold_cache.lock().unwrap();
        if let Some((cached_key, hidden)) = cache.get(&id) {
            if *cached_key == key {
                return hidden.clone();
            }
        }
        let rope = doc.rope();
        let mut ranges: Vec<(usize, usize)> = v
            .folds
            .iter()
            .filter(|f| **f <= doc.len())
            .filter_map(|f| {
                let line = doc.line_of(*f);
                (doc.line_offset(line) == *f).then_some(line)
            })
            .filter_map(|line| {
                smart::fold_end(
                    rope,
                    line,
                    self.preferences_for_document(v.document).indent_width,
                )
                .map(|end| (line + 1, end))
            })
            .collect();
        ranges.sort();
        let mut merged: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
        for (a, b) in ranges {
            match merged.last_mut() {
                Some(last) if a <= last.1 + 1 => last.1 = last.1.max(b),
                _ => merged.push((a, b)),
            }
        }
        let hidden = Hidden::new(merged);
        cache.insert(id, (key, hidden.clone()));
        hidden
    }

    fn hidden_range(hidden: &[(usize, usize)], line: usize) -> Option<(usize, usize)> {
        let i = hidden.partition_point(|(_, b)| *b < line);
        hidden.get(i).copied().filter(|(a, _)| *a <= line)
    }

    /// The first shown line at or after `line`.
    pub(crate) fn shown_at_or_after(&self, id: u64, line: usize) -> usize {
        let hidden = self.hidden_lines(id);
        Self::hidden_range(&hidden, line).map_or(line, |(_, b)| b + 1)
    }

    /// The next shown line after `line`, if any.
    pub(crate) fn next_shown_line(&self, id: u64, line: usize) -> Option<usize> {
        let lines = self.documents[&self.views[&id].document].line_count();
        let next = self.shown_at_or_after(id, line + 1);
        (next < lines).then_some(next)
    }

    /// The previous shown line before `line`, if any.
    pub(crate) fn previous_shown_line(&self, id: u64, line: usize) -> Option<usize> {
        let previous = line.checked_sub(1)?;
        let hidden = self.hidden_lines(id);
        Some(Self::hidden_range(&hidden, previous).map_or(previous, |(a, _)| a - 1))
    }

    /// Shown lines in `from..to` (exclusive of `to`).
    pub(crate) fn shown_between(&self, id: u64, from: usize, to: usize) -> usize {
        let hidden = self.hidden_lines(id);
        let covered: usize = hidden
            .iter()
            .map(|(a, b)| {
                let (a, b) = ((*a).max(from), (*b + 1).min(to));
                b.saturating_sub(a)
            })
            .sum();
        to.saturating_sub(from) - covered
    }

    /// Unfold whatever hides `line`.
    pub(crate) fn reveal_line(&mut self, id: u64, line: usize) {
        if self.views[&id].folds.is_empty()
            || Self::hidden_range(&self.hidden_lines(id), line).is_none()
        {
            return;
        }
        let doc = &self.documents[&self.views[&id].document];
        let tab = self.preferences.indent_width;
        let rope = doc.rope();
        let keep: Vec<usize> = self.views[&id]
            .folds
            .iter()
            .copied()
            .filter(|f| {
                let header = doc.line_of((*f).min(doc.len()));
                smart::fold_end(rope, header, tab).is_none_or(|end| !(header < line && line <= end))
            })
            .collect();
        self.views.get_mut(&id).unwrap().folds = keep;
    }

    /// Whether `line` is the header of a folded region of the view.
    pub(crate) fn folded_header(&self, id: u64, line: usize) -> bool {
        let v = &self.views[&id];
        if v.folds.is_empty() {
            return false;
        }
        let start = self.documents[&v.document].line_offset(line);
        v.folds.contains(&start) && Self::hidden_range(&self.hidden_lines(id), line + 1).is_some()
    }

    /// The header of the innermost foldable region containing `line`.
    fn fold_header_for(&self, id: u64, line: usize) -> Option<usize> {
        let doc = &self.documents[&self.views[&id].document];
        let tab = self.preferences.indent_width;
        let rope = doc.rope();
        if smart::foldable(rope, line, tab) {
            return Some(line);
        }
        (line.saturating_sub(5_000)..line)
            .rev()
            .find(|h| smart::fold_end(rope, *h, tab).is_some_and(|end| end >= line))
    }

    pub(crate) fn fold(&mut self, id: u64, fold: Option<bool>) -> Result<()> {
        let v = &self.views[&id];
        let doc = &self.documents[&v.document];
        let line = doc.line_of(v.cursor);
        let folded_here = self.folded_header(id, line);
        let unfold = fold.map_or(folded_here, |f| !f);
        if unfold {
            let start = doc.line_offset(line);
            let had = self.views[&id].folds.len();
            self.views
                .get_mut(&id)
                .unwrap()
                .folds
                .retain(|f| *f != start);
            if self.views[&id].folds.len() == had {
                // Unfold the region enclosing the caret instead.
                let header = self.fold_header_for(id, line).filter(|h| *h != line);
                let Some(header) = header else {
                    bail!("Nothing folded here");
                };
                let start = self.documents[&self.views[&id].document].line_offset(header);
                self.views
                    .get_mut(&id)
                    .unwrap()
                    .folds
                    .retain(|f| *f != start);
            }
            return Ok(());
        }
        let header = self
            .fold_header_for(id, line)
            .ok_or_else(|| anyhow::anyhow!("Nothing to fold here"))?;
        let start = doc.line_offset(header);
        let v = self.views.get_mut(&id).unwrap();
        if !v.folds.contains(&start) {
            v.folds.push(start);
        }
        // Keep the caret on the visible header.
        if header != line {
            v.cursor = start;
            v.anchor = None;
        }
        Ok(())
    }

    /// Fold every outermost region, or unfold everything.
    pub(crate) fn fold_all(&mut self, id: u64, fold: bool) {
        if !fold {
            self.views.get_mut(&id).unwrap().folds.clear();
            return;
        }
        let doc = &self.documents[&self.views[&id].document];
        let tab = self.preferences.indent_width;
        let rope = doc.rope();
        let mut folds = Vec::new();
        let mut line = 0;
        while line < doc.line_count() {
            match smart::fold_end(rope, line, tab) {
                Some(end) => {
                    folds.push(doc.line_offset(line));
                    line = end + 1;
                }
                None => line += 1,
            }
        }
        let cursor_line = doc.line_of(self.views[&id].cursor);
        self.views.get_mut(&id).unwrap().folds = folds;
        // The caret moves to the header that now hides it.
        let hidden = self.hidden_lines(id);
        if let Some((a, _)) = Self::hidden_range(&hidden, cursor_line) {
            let start = self.documents[&self.views[&id].document].line_offset(a - 1);
            let v = self.views.get_mut(&id).unwrap();
            v.cursor = start;
            v.anchor = None;
        }
    }
}
