//! Compact editor presentation. Terminal cell grids remain a separate frontend projection.
use crate::{terminal::Cell, App};
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TextSpan {
    pub start: usize,
    pub length: usize,
    pub fg: String,
    pub bg: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}
impl TextSpan {
    pub(super) fn cell(start: usize, length: usize, cell: &Cell) -> Self {
        Self {
            start,
            length,
            fg: cell.fg.clone(),
            bg: cell.bg.clone(),
            bold: cell.bold,
            italic: cell.italic,
            underline: cell.underline,
        }
    }
    fn same_style(&self, cell: &Cell) -> bool {
        self.fg == cell.fg
            && self.bg == cell.bg
            && self.bold == cell.bold
            && self.italic == cell.italic
            && self.underline == cell.underline
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TextLayout {
    pub text: String,
    // One terminal column per UTF-16 code unit, plus the end position. This keeps
    // shaped Qt text, Rust grapheme positions, mouse input and IME in agreement.
    pub columns: Vec<u16>,
    pub formats: Vec<TextSpan>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TextLine {
    pub layout: TextLayout,
    pub overlays: Vec<TextSpan>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EditorText {
    pub document: u64,
    pub top: usize,
    pub left: usize,
    pub highlighted: bool,
    pub cursor: Option<(u16, u16)>,
    pub lines: Vec<TextLine>,
}
impl App {
    pub(super) fn editor_text(&mut self, id: u64, rows: u16, cols: u16) -> EditorText {
        let (screen, decorations) = self.render_editor(id, rows, cols, true);
        let lines = screen
            .cells
            .iter()
            .zip(decorations)
            .map(|(cells, mut overlays)| {
                let mut text = String::new();
                let mut columns = Vec::new();
                let mut formats: Vec<TextSpan> = Vec::new();
                for (col, cell) in cells.iter().enumerate() {
                    // An orphaned continuation at the left clip edge occupies a
                    // blank column. Otherwise the preceding wide grapheme owns it.
                    if cell.continuation && col > 0 && cells[col - 1].wide {
                        continue;
                    }
                    let glyph = if cell.continuation || cell.text.is_empty() {
                        " "
                    } else {
                        &cell.text
                    };
                    let length = glyph.encode_utf16().count();
                    if let Some(last) = formats.last_mut().filter(|s| s.same_style(cell)) {
                        last.length += length;
                    } else {
                        formats.push(TextSpan::cell(columns.len(), length, cell));
                    }
                    columns.extend(std::iter::repeat_n(col as u16, length));
                    text.push_str(glyph);
                }
                columns.push(cells.len() as u16);
                for overlay in &mut overlays {
                    let start = columns.partition_point(|c| usize::from(*c) < overlay.start);
                    let end = columns
                        .partition_point(|c| usize::from(*c) < overlay.start + overlay.length);
                    overlay.start = start;
                    overlay.length = end - start;
                }
                TextLine {
                    layout: TextLayout {
                        text,
                        columns,
                        formats,
                    },
                    overlays,
                }
            })
            .collect();
        let view = &self.views[&id];
        EditorText {
            document: view.document,
            top: view.top,
            left: view.left,
            highlighted: self
                .highlights
                .get(&view.document)
                .is_some_and(|cache| cache.current(view.top, view.top + view.rows as usize)),
            cursor: screen.cursor,
            lines,
        }
    }
}
