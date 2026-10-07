//! Visual rows of soft-wrapped lines, shared by rendering, hit testing,
//! scrolling and vertical cursor movement.
use crate::document::grapheme_width;
use unicode_segmentation::UnicodeSegmentation;

/// One visual row of a line: a byte range within the line and the display
/// column (measured from the line start) at which it begins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub start: usize,
    pub end: usize,
    pub col: usize,
}

fn cells(line: &str, tab: usize) -> Box<dyn Iterator<Item = (usize, usize, bool)> + '_> {
    // (byte offset, grapheme byte length, is whitespace)
    if line.is_ascii() {
        Box::new(
            line.bytes()
                .enumerate()
                .map(|(i, b)| (i, 1, b == b' ' || b == b'\t')),
        )
    } else {
        let _ = tab;
        Box::new(
            line.grapheme_indices(true)
                .map(|(i, g)| (i, g.len(), g.chars().all(char::is_whitespace))),
        )
    }
}

/// Split a line (without its line break) into rows at most `width` columns
/// wide, breaking after whitespace when a row contains any.
pub fn rows(line: &str, width: usize, tab: usize) -> Vec<Row> {
    let width = width.max(1);
    let tab = tab.max(1);
    let mut rows = vec![];
    let mut row = Row {
        start: 0,
        end: line.len(),
        col: 0,
    };
    let mut x = 0;
    // Last break opportunity within the current row: (byte, column).
    let mut breakpoint: Option<(usize, usize)> = None;
    for (i, len, space) in cells(line, tab) {
        let g = &line[i..i + len];
        let w = grapheme_width(g, x, tab);
        if x + w - row.col > width && i > row.start {
            let (start, col) = breakpoint
                .filter(|(b, _)| *b > row.start && *b <= i)
                .unwrap_or((i, x));
            row.end = start;
            rows.push(row);
            row = Row {
                start,
                end: line.len(),
                col,
            };
            breakpoint = None;
        }
        x += w;
        if space {
            breakpoint = Some((i + len, x));
        }
    }
    rows.push(row);
    rows
}

/// Index of the row that displays a byte offset. A position at a row
/// boundary belongs to the following row, except at the end of the line.
pub fn row_of(rows: &[Row], offset: usize) -> usize {
    rows.iter().rposition(|r| r.start <= offset).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_at_whitespace_and_hard_breaks_long_words() {
        let r = rows("hello world again", 11, 4);
        let text: Vec<_> = r
            .iter()
            .map(|r| &"hello world again"[r.start..r.end])
            .collect();
        assert_eq!(text, ["hello ", "world again"]);
        assert_eq!(r[1].col, 6);
        let r = rows("abcdefghij", 4, 4);
        assert_eq!(r.len(), 3);
        assert_eq!((r[2].start, r[2].col), (8, 8));
        assert_eq!(
            rows("", 10, 4),
            [Row {
                start: 0,
                end: 0,
                col: 0
            }]
        );
        let wide = rows("猫猫猫", 4, 4);
        assert_eq!(wide.len(), 2);
        assert_eq!(row_of(&wide, 6), 1);
        assert_eq!(row_of(&wide, 9), 1);
    }
}
