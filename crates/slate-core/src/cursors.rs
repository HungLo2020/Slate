//! Multiple cursors and the smart-typing path shared with single cursors.
//!
//! A view's primary caret stays in `cursor`/`anchor`; additional carets live
//! in `extra`. Edits from all carets apply as one undoable change.
use crate::{smart, App, Key};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// The most carets one view keeps (select-all-occurrences stops here).
pub const MAX_CURSORS: usize = 10_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub cursor: usize,
    pub anchor: Option<usize>,
}
impl Selection {
    pub fn caret(cursor: usize) -> Self {
        Self {
            cursor,
            anchor: None,
        }
    }
    pub fn range(&self) -> (usize, usize) {
        let anchor = self.anchor.unwrap_or(self.cursor);
        (self.cursor.min(anchor), self.cursor.max(anchor))
    }
}

impl App {
    /// All carets of a view, primary first.
    pub(crate) fn selections(&self, id: u64) -> Vec<Selection> {
        let v = &self.views[&id];
        let mut all = Vec::with_capacity(1 + v.extra.len());
        all.push(Selection {
            cursor: v.cursor,
            anchor: v.anchor,
        });
        all.extend(v.extra.iter().copied());
        all
    }

    /// Store carets (the first is primary), merging ones that overlap.
    pub(crate) fn set_selections(&mut self, id: u64, selections: Vec<Selection>) {
        let Some(primary) = selections.first().copied() else {
            return;
        };
        let mut sorted = selections;
        sorted.sort_by_key(|s| s.range());
        let mut merged: Vec<Selection> = Vec::with_capacity(sorted.len());
        let mut primary_index = 0;
        for s in sorted {
            let is_primary = s == primary;
            match merged.last_mut() {
                Some(last)
                    if {
                        let (a, b) = last.range();
                        let (c, d) = s.range();
                        c < b || (c == b && (a == b || c == d)) || (a, b) == (c, d)
                    } =>
                {
                    let (a, b) = last.range();
                    let (_, d) = s.range();
                    let end = b.max(d);
                    *last = if a == end {
                        Selection::caret(end)
                    } else {
                        Selection {
                            cursor: end,
                            anchor: Some(a),
                        }
                    };
                    if is_primary {
                        primary_index = merged.len() - 1;
                    }
                }
                _ => {
                    if is_primary {
                        primary_index = merged.len();
                    }
                    merged.push(s);
                }
            }
        }
        merged.truncate(MAX_CURSORS);
        let primary_index = primary_index.min(merged.len() - 1);
        let primary = merged.remove(primary_index);
        let v = self.views.get_mut(&id).unwrap();
        v.cursor = primary.cursor;
        v.anchor = primary.anchor;
        v.extra = merged;
        v.goal = None;
        v.manual_scroll = false;
    }

    /// Apply one replacement per caret as a single undoable change.
    pub(crate) fn apply_typed(&mut self, id: u64, typed: Vec<smart::Typed>) -> Result<()> {
        let doc = self.views[&id].document;
        let before = self.views[&id].cursor;
        let edits: Vec<(usize, usize, String)> = typed
            .iter()
            .map(|t| (t.start, t.end, t.text.clone()))
            .collect();
        let placed = self
            .documents
            .get_mut(&doc)
            .unwrap()
            .replace_many(&edits, before)?;
        // Other views (and folds) follow every replacement. From the end,
        // each range is still in the coordinates of the original text.
        let mut order: Vec<usize> = (0..edits.len()).collect();
        order.sort_by_key(|i| std::cmp::Reverse(edits[*i].0));
        for i in order {
            let (start, end, text) = &edits[i];
            self.rebase_views(doc, *start, *end, text.len());
        }
        let selections = typed
            .iter()
            .zip(placed)
            .map(|(t, (start, _))| Selection {
                cursor: start + t.caret,
                anchor: t.anchor.map(|a| start + a),
            })
            .collect();
        self.set_selections(id, selections);
        self.views.get_mut(&id).unwrap().mark = false;
        Ok(())
    }

    /// Move every caret of a view.
    pub(crate) fn multi_move(&mut self, id: u64, name: &str, extend: bool) -> Result<()> {
        let selections = self.selections(id);
        let mut moved = Vec::with_capacity(selections.len());
        for s in selections {
            {
                let v = self.views.get_mut(&id).unwrap();
                v.cursor = s.cursor;
                v.anchor = s.anchor;
                v.goal = None;
                v.extra.clear();
            }
            self.move_cursor(id, name, extend)?;
            let v = &self.views[&id];
            moved.push(Selection {
                cursor: v.cursor,
                anchor: v.anchor,
            });
        }
        self.set_selections(id, moved);
        Ok(())
    }

    /// Editing keys with several carets. Returns false for keys it leaves
    /// to the normal path.
    pub(crate) fn multi_key(&mut self, id: u64, k: &Key) -> Result<bool> {
        let unit = self.indent_unit();
        let pairs = self.preferences.auto_close_brackets;
        let auto_indent = self.preferences.auto_indent;
        let selections = self.selections(id);
        let doc = &self.documents[&self.views[&id].document];
        let rope = doc.rope();
        let typed: Vec<smart::Typed> = match k.key.as_str() {
            "Escape" => {
                let v = self.views.get_mut(&id).unwrap();
                v.extra.clear();
                return Ok(true);
            }
            "Backspace" | "Delete" => {
                let forward = k.key == "Delete";
                selections
                    .iter()
                    .map(|s| {
                        let (start, end) = s.range();
                        if start != end {
                            return smart::Typed::plain(start, end, "");
                        }
                        if k.ctrl {
                            let other =
                                crate::actions::document_word_boundary(doc, s.cursor, forward);
                            let (a, b) = (other.min(s.cursor), other.max(s.cursor));
                            return smart::Typed::plain(a, b, "");
                        }
                        if !forward && pairs {
                            if let Some((a, b)) = smart::pair_around(rope, s.cursor) {
                                return smart::Typed::plain(a, b, "");
                            }
                        }
                        if forward {
                            smart::Typed::plain(s.cursor, doc.next_boundary(s.cursor), "")
                        } else {
                            smart::Typed::plain(doc.previous_boundary(s.cursor), s.cursor, "")
                        }
                    })
                    .collect()
            }
            "Enter" => selections
                .iter()
                .map(|s| {
                    let (start, end) = s.range();
                    smart::enter(rope, start, end, &unit, auto_indent)
                })
                .collect(),
            "Tab" if !k.shift => selections
                .iter()
                .map(|s| {
                    let (start, end) = s.range();
                    let (_, col) = doc.line_col(start, self.preferences.indent_width);
                    let text = if self.preferences.insert_spaces {
                        let width = self.preferences.indent_width;
                        " ".repeat(width - col % width)
                    } else {
                        "\t".into()
                    };
                    smart::Typed::plain(start, end, &text)
                })
                .collect(),
            _ if !k.ctrl && !k.alt && !k.text.is_empty() => {
                let text = crate::text_format::normalize_input(&k.text).into_owned();
                selections
                    .iter()
                    .map(|s| self.typed_at(id, *s, &text))
                    .collect()
            }
            _ => return Ok(false),
        };
        self.apply_typed(id, typed)?;
        Ok(true)
    }

    /// What typing `text` at one caret does, with pairing and closer dedent.
    pub(crate) fn typed_at(&self, id: u64, s: Selection, text: &str) -> smart::Typed {
        let doc = &self.documents[&self.views[&id].document];
        let (start, end) = s.range();
        let mut chars = text.chars();
        if let (Some(c), None, true) = (chars.next(), chars.next(), start == end) {
            // Stepping over an existing closer wins over re-indenting.
            let steps_over = self.preferences.auto_close_brackets
                && doc.slice(start, doc.next_boundary(start)) == text;
            if self.preferences.auto_indent && !steps_over {
                if let Some(t) = smart::dedent_closer(doc.rope(), start, c) {
                    return t;
                }
            }
        }
        smart::type_text(
            doc.rope(),
            start,
            end,
            text,
            self.preferences.auto_close_brackets,
        )
    }

    /// One indentation level as text.
    pub(crate) fn indent_unit(&self) -> String {
        if self.preferences.insert_spaces {
            " ".repeat(self.preferences.indent_width)
        } else {
            "\t".into()
        }
    }

    /// Paste at every caret: one line each when the clipboard holds exactly
    /// one line per caret, otherwise the whole text at each.
    pub(crate) fn multi_paste(&mut self, id: u64, text: &str) -> Result<()> {
        let selections = self.selections(id);
        let lines: Vec<&str> = text.trim_end_matches('\n').split('\n').collect();
        let distribute = lines.len() == selections.len();
        // Lines go to carets in document order; the primary stays first.
        let mut order: Vec<usize> = (0..selections.len()).collect();
        order.sort_by_key(|i| selections[*i].range());
        let mut rank = vec![0; selections.len()];
        for (r, i) in order.into_iter().enumerate() {
            rank[i] = r;
        }
        let typed = selections
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let (start, end) = s.range();
                smart::Typed::plain(start, end, if distribute { lines[rank[i]] } else { text })
            })
            .collect();
        self.apply_typed(id, typed)
    }

    /// The selected text of every caret, one per line, for copying.
    pub(crate) fn selections_text(&self, id: u64) -> String {
        let doc = &self.documents[&self.views[&id].document];
        let mut selections = self.selections(id);
        selections.sort_by_key(|s| s.range());
        selections
            .iter()
            .map(|s| {
                let (a, b) = s.range();
                doc.slice(a, b).into_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Add a caret on the line above (`delta < 0`) or below the outermost
    /// caret in that direction.
    pub(crate) fn add_cursor_vertical(&mut self, id: u64, delta: isize) -> Result<()> {
        let mut selections = self.selections(id);
        let edge = if delta < 0 {
            selections.iter().map(|s| s.cursor).min()
        } else {
            selections.iter().map(|s| s.cursor).max()
        }
        .unwrap_or(0);
        let (_, _, col) = self.visual_position(id, edge);
        let target = self.vertical(id, edge, delta, col);
        if target == edge || selections.iter().any(|s| s.cursor == target) {
            bail!(
                "No line {} to add a cursor",
                if delta < 0 { "above" } else { "below" }
            );
        }
        selections.insert(0, Selection::caret(target));
        self.set_selections(id, selections);
        self.status = format!("{} cursors", self.selections(id).len());
        Ok(())
    }

    /// Add a caret at `offset` (Alt+click), or remove the caret there.
    pub(crate) fn toggle_cursor_at(&mut self, id: u64, offset: usize) {
        let mut selections = self.selections(id);
        if let Some(index) = selections.iter().position(|s| s.cursor == offset) {
            if selections.len() > 1 {
                selections.remove(index);
                self.set_selections(id, selections);
            }
            return;
        }
        selections.insert(0, Selection::caret(offset));
        self.set_selections(id, selections);
    }

    /// The word around a caret, as a byte range.
    fn word_at(&self, id: u64, offset: usize) -> Option<(usize, usize)> {
        let doc = &self.documents[&self.views[&id].document];
        let line = doc.line_of(offset);
        let (start, _) = doc.line_range(line);
        let text = doc.line_text(line);
        let column = offset - start;
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let left = text[..column]
            .char_indices()
            .rev()
            .take_while(|(_, c)| is_word(*c))
            .last()
            .map_or(column, |(i, _)| i);
        let right = text[column..]
            .char_indices()
            .find(|(_, c)| !is_word(*c))
            .map_or(text.trim_end_matches(['\n', '\r']).len(), |(i, _)| {
                column + i
            });
        (left < right).then_some((start + left, start + right))
    }

    /// Ctrl+D: select the word at the caret, then add the next occurrence of
    /// the selection as another caret.
    pub(crate) fn add_next_occurrence(&mut self, id: u64) -> Result<()> {
        let selections = self.selections(id);
        let primary = selections[0];
        let (start, end) = primary.range();
        if start == end {
            let (a, b) = self
                .word_at(id, start)
                .ok_or_else(|| anyhow::anyhow!("No word at the cursor"))?;
            let mut all = selections;
            all[0] = Selection {
                cursor: b,
                anchor: Some(a),
            };
            self.set_selections(id, all);
            return Ok(());
        }
        let doc = &self.documents[&self.views[&id].document];
        let needle = doc.slice(start, end).into_owned();
        let after = selections.iter().map(|s| s.range().1).max().unwrap_or(end);
        let text = doc.text();
        let found = text[after..]
            .find(&needle)
            .map(|i| after + i)
            .or_else(|| text.find(&needle))
            .filter(|at| !selections.iter().any(|s| s.range().0 == *at));
        let Some(at) = found else {
            bail!("No more occurrences");
        };
        let mut all = selections;
        all.insert(
            0,
            Selection {
                cursor: at + needle.len(),
                anchor: Some(at),
            },
        );
        self.set_selections(id, all);
        self.status = format!("{} cursors", self.selections(id).len());
        Ok(())
    }

    /// Select every occurrence of the selection (or the word at the caret).
    pub(crate) fn select_all_occurrences(&mut self, id: u64) -> Result<()> {
        let primary = self.selections(id)[0];
        let (mut start, mut end) = primary.range();
        if start == end {
            (start, end) = self
                .word_at(id, start)
                .ok_or_else(|| anyhow::anyhow!("No word at the cursor"))?;
        }
        let doc = &self.documents[&self.views[&id].document];
        let needle = doc.slice(start, end).into_owned();
        let text = doc.text();
        let mut all: Vec<Selection> = text
            .match_indices(&needle)
            .take(MAX_CURSORS)
            .map(|(at, _)| Selection {
                cursor: at + needle.len(),
                anchor: Some(at),
            })
            .collect();
        // Keep the occurrence under the original caret primary.
        if let Some(index) = all.iter().position(|s| s.range().0 == start) {
            all.swap(0, index);
        }
        let count = all.len();
        self.set_selections(id, all);
        self.status = format!("{count} occurrences selected");
        Ok(())
    }
}
