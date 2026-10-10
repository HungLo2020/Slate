//! Jumping to locations (search results, symbols, definitions, diagnostics)
//! and the back/forward history those jumps leave behind.
use crate::{layout::View, App, EditorView};
use anyhow::Result;
use std::path::{Path, PathBuf};

/// A column as different sources count it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    /// Display columns, as people and `slate FILE:LINE:COL` count them.
    Display(usize),
    /// UTF-16 code units, as the Language Server Protocol counts them.
    Utf16(usize),
    /// Bytes from the line start.
    Byte(usize),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    /// Zero-based line.
    pub line: usize,
    pub column: Column,
}

/// Jumps remembered for go-back / go-forward.
const HISTORY: usize = 100;

pub(crate) fn same_file(a: &Path, b: &Path) -> bool {
    a == b
        || match (a.canonicalize(), b.canonicalize()) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
}

impl App {
    /// The byte offset of a line and column in a document, clamped to it.
    pub(crate) fn offset_for(&self, doc: u64, line: usize, column: Column) -> usize {
        let d = &self.documents[&doc];
        let line = line.min(d.line_count().saturating_sub(1));
        match column {
            Column::Display(col) => d.at_line_col(line, col, self.preferences.indent_width),
            Column::Utf16(units) => d.offset_of(line, units),
            Column::Byte(bytes) => {
                let (start, end) = d.line_range(line);
                d.floor_boundary((start + bytes).min(end))
            }
        }
    }

    /// The open document for a path, if any. Document paths are already
    /// resolved (opening and saving canonicalize them), so only the path
    /// asked about is resolved, once, rather than every open document's on
    /// each lookup (diagnostics and reference lists look up many paths).
    pub(crate) fn document_for(&self, path: &Path) -> Option<u64> {
        let resolved = path.canonicalize().ok();
        self.documents
            .iter()
            .find(|(_, d)| {
                d.path
                    .as_deref()
                    .is_some_and(|p| p == path || resolved.as_deref() == Some(p))
            })
            .map(|(id, _)| *id)
    }

    /// Focus a view of `doc`, reusing a tab that shows it.
    pub(crate) fn reveal_document(&mut self, doc: u64) -> u64 {
        if let Some(View::Editor(id)) = self.layout.view(self.focus) {
            if self.views[id].document == doc {
                return *id;
            }
        }
        for pane in self.layout.panes() {
            let (tabs, active) = self.layout.pane_mut(pane).unwrap();
            if let Some(index) = tabs
                .iter()
                .position(|v| matches!(v, View::Editor(id) if self.views.get(id).is_some_and(|v| v.document == doc)))
            {
                *active = index;
                self.focus = pane;
                if let View::Editor(id) = tabs[index] {
                    return id;
                }
            }
        }
        self.editor_target();
        let id = self.id();
        self.views.insert(
            id,
            EditorView {
                document: doc,
                ..Default::default()
            },
        );
        self.add_tab(View::Editor(id));
        id
    }

    /// Remember where the caret is before a jump.
    pub(crate) fn remember_position(&mut self) {
        if let Some(id) = self.active_editor() {
            let v = &self.views[&id];
            let entry = (v.document, v.cursor);
            if self.back.last() != Some(&entry) {
                self.back.push(entry);
                if self.back.len() > HISTORY {
                    self.back.remove(0);
                }
            }
            self.forward.clear();
        }
    }

    fn place_caret(&mut self, view: u64, offset: usize) {
        let v = self.views.get_mut(&view).unwrap();
        v.cursor = offset;
        v.anchor = None;
        v.extra.clear();
        v.manual_scroll = false;
        v.goal = None;
    }

    /// Jump to a location, opening its file when needed.
    pub fn goto(&mut self, location: Location) -> Result<()> {
        self.remember_position();
        if let Some(doc) = self.document_for(&location.path) {
            let view = self.reveal_document(doc);
            let offset = self.offset_for(doc, location.line, location.column);
            self.place_caret(view, offset);
            return Ok(());
        }
        self.pending_positions
            .insert(location.path.clone(), (location.line, location.column));
        self.execute(crate::Command::Open {
            path: location.path,
        })
    }

    /// Return to the position before the last jump (`forward` = redo).
    pub(crate) fn go_back(&mut self, forward: bool) -> Result<()> {
        let current = self.active_editor().map(|id| {
            let v = &self.views[&id];
            (v.document, v.cursor)
        });
        let (from, to) = if forward {
            (&mut self.forward, &mut self.back)
        } else {
            (&mut self.back, &mut self.forward)
        };
        // Positions in documents closed since are skipped.
        let documents = &self.documents;
        while from
            .last()
            .is_some_and(|(doc, _)| !documents.contains_key(doc))
        {
            from.pop();
        }
        let Some((doc, offset)) = from.pop() else {
            anyhow::bail!("No {} position", if forward { "next" } else { "previous" });
        };
        to.extend(current);
        let view = self.reveal_document(doc);
        let offset = self.documents[&doc].floor_boundary(offset.min(self.documents[&doc].len()));
        self.place_caret(view, offset);
        Ok(())
    }
}
