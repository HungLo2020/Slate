//! Assistive technology uses document-wide UTF-16 offsets, independent of IME windows.
use crate::{layout::View, App};
use anyhow::{Context, Result};
use serde_json::{json, Value};
impl App {
    fn accessible_view(&self, pane: u64) -> Option<u64> {
        match self.layout.view(pane) {
            Some(View::Editor(id)) => Some(*id),
            _ => None,
        }
    }
    pub fn accessible_context(&self, pane: u64) -> Value {
        let Some(id) = self.accessible_view(pane) else {
            return json!({});
        };
        let view = &self.views[&id];
        let doc = &self.documents[&view.document];
        json!({"length": doc.rope().len_utf16_cu(), "cursor": doc.utf16_offset(view.cursor), "anchor": doc.utf16_offset(view.anchor.unwrap_or(view.cursor)), "document": view.document, "generation": doc.generation})
    }
    pub fn accessible_text(&self, pane: u64, start: usize, end: usize) -> Option<String> {
        let id = self.accessible_view(pane)?;
        let doc = &self.documents[&self.views[&id].document];
        let start = doc.byte_of_utf16(start.min(doc.rope().len_utf16_cu()))?;
        let end = doc.byte_of_utf16(end.min(doc.rope().len_utf16_cu()))?;
        Some(doc.slice(start.min(end), end).into_owned())
    }
    pub fn accessible_position(&self, pane: u64, offset: usize) -> Value {
        let Some(id) = self.accessible_view(pane) else {
            return json!({});
        };
        let view = &self.views[&id];
        let doc = &self.documents[&view.document];
        let Some(byte) = doc.byte_of_utf16(offset) else {
            return json!({});
        };
        let options = self.preferences_for_document(view.document);
        let (line, column) = doc.line_col(byte, options.indent_width);
        for (y, (logical, _, row)) in self.visible_rows(id, view.rows as usize).iter().enumerate() {
            let start = doc.line_offset(*logical);
            if *logical == line && byte >= start + row.start && byte <= start + row.end {
                let left = row.col + if options.soft_wrap { 0 } else { view.left };
                if column < left {
                    return json!({});
                }
                let x = column - left + self.gutter_width(view.document, view.cols);
                if x >= view.cols as usize {
                    return json!({});
                }
                return json!({"row": y, "col": x});
            }
        }
        json!({})
    }
    pub fn accessible_offset(&self, pane: u64, row: usize, col: usize) -> Option<usize> {
        let id = self.accessible_view(pane)?;
        Some(
            self.documents[&self.views[&id].document].utf16_offset(self.click_offset(id, row, col)),
        )
    }
    pub(crate) fn accessible_select(&mut self, pane: u64, start: usize, end: usize) -> Result<()> {
        let id = self
            .accessible_view(pane)
            .context("No editor in this pane")?;
        let doc = &self.documents[&self.views[&id].document];
        let anchor = doc
            .byte_of_utf16(start)
            .context("Invalid selection start")?;
        let cursor = doc.byte_of_utf16(end).context("Invalid selection end")?;
        self.focus = pane;
        let view = self.views.get_mut(&id).unwrap();
        view.anchor = (anchor != cursor).then_some(anchor);
        view.cursor = cursor;
        view.extra.clear();
        view.manual_scroll = false;
        self.ensure_visible(id);
        Ok(())
    }
    pub(crate) fn accessible_scroll(&mut self, pane: u64, offset: usize) -> Result<()> {
        let id = self
            .accessible_view(pane)
            .context("No editor in this pane")?;
        let doc = &self.documents[&self.views[&id].document];
        let byte = doc.byte_of_utf16(offset).context("Invalid text offset")?;
        let (line, row, _) = self.visual_position(id, byte);
        let view = self.views.get_mut(&id).unwrap();
        view.top = line;
        view.top_row = row;
        view.manual_scroll = true;
        Ok(())
    }
}
