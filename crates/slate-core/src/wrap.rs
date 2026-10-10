//! Visual rows of soft-wrapped lines, shared by rendering, hit testing,
//! scrolling and vertical cursor movement.
use crate::document::{grapheme_width, Document, Splice};
use ropey::RopeSlice;
use std::{borrow::Cow, collections::HashMap, sync::Arc};
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
                // Classified like the grapheme path below, so a re-wrapped window
                // agrees with the whole line whichever path each takes.
                .map(|(i, b)| (i, 1, char::from(b).is_whitespace())),
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
#[cfg(test)]
pub fn rows(line: &str, width: usize, tab: usize) -> Vec<Row> {
    Wrapped::new(line, width, tab).rows.to_vec()
}

/// Bytes past an edit scanned for the rows to line up again before the
/// rest of the line is read.
const WINDOW: usize = 64 * 1024;

/// A wrapped line and where scanning stood when each row began, so an edit
/// re-wraps only the rows it can change.
#[derive(Clone)]
pub(crate) struct Wrapped {
    pub rows: Arc<Vec<Row>>,
    /// Per row: the cell whose overflow began it and the column before it.
    resume: Vec<(usize, usize)>,
    /// The line may contain a tab, whose width depends on its column.
    tabs: bool,
}

/// Continue wrapping with `row` open, at line byte `base` (the start of
/// `text`) and column `x`. `resumed` means the cell at `base` already broke
/// `row` off the previous one. Returns true when `converged` accepts a new
/// row (given it, its cell, the column before it and the cell's end); that
/// row is then left to the caller.
#[allow(clippy::too_many_arguments)]
fn scan(
    text: &str,
    base: usize,
    len: usize,
    mut row: Row,
    mut x: usize,
    resumed: bool,
    width: usize,
    tab: usize,
    rows: &mut Vec<Row>,
    resume: &mut Vec<(usize, usize)>,
    mut converged: impl FnMut(&Row, usize, usize, usize) -> bool,
) -> bool {
    // Last break opportunity within the current row: (byte, column).
    let mut breakpoint: Option<(usize, usize)> = None;
    let mut check = !resumed;
    for (j, size, space) in cells(text, tab) {
        let i = base + j;
        let w = grapheme_width(&text[j..j + size], x, tab);
        if check && x + w - row.col > width && i > row.start {
            let (start, col) = breakpoint
                .filter(|(b, _)| *b > row.start && *b <= i)
                .unwrap_or((i, x));
            row.end = start;
            rows.push(row);
            row = Row {
                start,
                end: len,
                col,
            };
            breakpoint = None;
            if converged(&row, i, x, i + size) {
                return true;
            }
            resume.push((i, x));
        }
        check = true;
        x += w;
        if space {
            breakpoint = Some((i + size, x));
        }
    }
    row.end = len;
    rows.push(row);
    false
}

impl Wrapped {
    pub(crate) fn new(line: &str, width: usize, tab: usize) -> Self {
        let (width, tab) = (width.max(1), tab.max(1));
        let mut rows = vec![];
        let mut resume = vec![(0, 0)];
        let first = Row {
            start: 0,
            end: line.len(),
            col: 0,
        };
        let never = |_: &Row, _, _, _| false;
        scan(
            line,
            0,
            line.len(),
            first,
            0,
            false,
            width,
            tab,
            &mut rows,
            &mut resume,
            never,
        );
        Self {
            rows: Arc::new(rows),
            resume,
            tabs: line.contains('\t'),
        }
    }

    /// The rows after the bytes `lo..hi` of the wrapped text became
    /// `lo..hi + delta` of `line` (the whole new line, without its break).
    /// Rows before the change are kept, and once a row begins at the same
    /// place in the unchanged text after it, the rest are shifted copies.
    pub(crate) fn edited(
        &self,
        line: RopeSlice,
        lo: usize,
        hi: usize,
        delta: isize,
        width: usize,
        tab: usize,
    ) -> Self {
        let (width, tab) = (width.max(1), tab.max(1));
        let len = line.len_bytes();
        let changed_end = hi.saturating_add_signed(delta);
        // The row before the last one begun before the change: its break
        // decision may read the cell the change touches.
        let begun = self.resume.partition_point(|(cell, _)| *cell < lo);
        let restart = begun.saturating_sub(2);
        let (cell, x) = self.resume[restart];
        let row = Row {
            start: self.rows[restart].start,
            end: len,
            col: self.rows[restart].col,
        };
        let text = |from: usize, to: usize| -> Cow<'_, str> {
            let slice = line.byte_slice(from..to);
            match slice.as_str() {
                Some(s) => Cow::Borrowed(s),
                None => Cow::Owned(slice.to_string()),
            }
        };
        let mut tabs = self.tabs;
        let mut attempt = |to: usize| {
            let mut rows = self.rows[..restart].to_vec();
            let mut resume = self.resume[..=restart].to_vec();
            let window = text(cell, to);
            tabs |= window[..changed_end.clamp(cell, to) - cell].contains('\t');
            let tail = std::cell::Cell::new(None);
            let converged = |row: &Row, i: usize, x: usize, end: usize| {
                if row.start < changed_end || end >= to && to < len {
                    return false;
                }
                let old_start = row.start.checked_add_signed(-delta);
                let Some(m) =
                    old_start.and_then(|s| self.rows.binary_search_by_key(&s, |r| r.start).ok())
                else {
                    return false;
                };
                let (old_cell, old_x) = self.resume[m];
                let old = self.rows[m];
                let dcol = row.col as isize - old.col as isize;
                let lined_up = i.checked_add_signed(-delta) == Some(old_cell)
                    && x - row.col == old_x - old.col
                    && (!self.tabs || dcol.rem_euclid(tab as isize) == 0);
                if lined_up {
                    tail.set(Some((m, dcol)));
                }
                lined_up
            };
            let done = scan(
                &window,
                cell,
                len,
                row,
                x,
                restart > 0,
                width,
                tab,
                &mut rows,
                &mut resume,
                converged,
            );
            if let (true, Some((m, dcol))) = (done, tail.get()) {
                let shift = |n: usize, by: isize| n.saturating_add_signed(by);
                rows.extend(self.rows[m..].iter().map(|r| Row {
                    start: shift(r.start, delta),
                    end: shift(r.end, delta),
                    col: shift(r.col, dcol),
                }));
                resume.extend(
                    self.resume[m..]
                        .iter()
                        .map(|(i, x)| (shift(*i, delta), shift(*x, dcol))),
                );
                return Some((rows, resume));
            }
            (to == len).then_some((rows, resume))
        };
        // Usually the rows line up again soon after the change.
        let near =
            line.char_to_byte(line.byte_to_char(changed_end.saturating_add(WINDOW).min(len)));
        let (rows, resume) = attempt(near).or_else(|| attempt(len)).unwrap();
        Self {
            rows: Arc::new(rows),
            resume,
            tabs,
        }
    }
}

/// Lines kept wrapped across all documents, and rows they may hold (the
/// line being used is kept even when it alone holds more).
const ENTRIES: usize = 128;
const ROWS: usize = 131_072;

/// Wrapped lines shared across cursor movement and rendering. They follow
/// edits: lines an edit does not touch keep their rows, and a line edited
/// within re-wraps only around the change.
#[derive(Default)]
pub(crate) struct Cache {
    docs: HashMap<u64, Lines>,
}
#[derive(Default)]
struct Lines {
    /// The document text the entries describe.
    serial: u64,
    /// By (line, width, tab width).
    entries: HashMap<(usize, usize, usize), Entry>,
}
struct Entry {
    start: usize,
    len: usize,
    wrapped: Wrapped,
    /// Bytes `lo..hi` of the wrapped text since became `lo..hi + delta`.
    changed: Option<(usize, usize, isize)>,
}
impl Entry {
    /// Follow a splice; false when it changes the line's extent.
    fn follow(&mut self, line: &mut usize, splice: &Splice) -> bool {
        let delta = splice.new_len as isize - (splice.old_end - splice.start) as isize;
        if splice.old_end < self.start {
            self.start = self.start.saturating_add_signed(delta);
            *line = (*line + splice.new_breaks).saturating_sub(splice.old_breaks);
            return true;
        }
        if splice.start > self.start + self.len {
            return true;
        }
        if splice.old_breaks > 0 || splice.new_breaks > 0 {
            return false;
        }
        // Within the line: widen the changed span to cover this splice.
        let (a, b) = (splice.start - self.start, splice.old_end - self.start);
        self.changed = Some(match self.changed {
            None => (a, b, delta),
            Some((lo, hi, before)) => {
                let end = hi.saturating_add_signed(before).max(b);
                (
                    lo.min(a),
                    end.saturating_add_signed(-before),
                    before + delta,
                )
            }
        });
        self.len = self.len.saturating_add_signed(delta);
        true
    }
}
impl Lines {
    /// Carry the entries to the document's current text.
    fn follow(&mut self, d: &Document) {
        let serial = d.serial();
        let Some(splices) = d.splices_since(self.serial) else {
            self.entries.clear();
            self.serial = serial;
            return;
        };
        let splices: Vec<&Splice> = splices.collect();
        if !splices.is_empty() {
            self.entries = std::mem::take(&mut self.entries)
                .into_iter()
                .filter_map(|((mut line, width, tab), mut entry)| {
                    splices
                        .iter()
                        .all(|splice| entry.follow(&mut line, splice))
                        .then_some(((line, width, tab), entry))
                })
                .collect();
        }
        self.serial = serial;
    }
}
impl Cache {
    /// Drop a closed document's lines.
    pub(crate) fn forget(&mut self, doc: u64) {
        self.docs.remove(&doc);
    }
    pub(crate) fn rows(
        &mut self,
        doc: u64,
        d: &Document,
        line: usize,
        width: usize,
        tab: usize,
    ) -> Arc<Vec<Row>> {
        let lines = self.docs.entry(doc).or_default();
        if lines.serial != d.serial() {
            lines.follow(d);
        }
        let (start, end) = d.line_range(line);
        let key = (line, width, tab);
        if let Some(entry) = lines
            .entries
            .get_mut(&key)
            .filter(|e| e.start == start && e.len == end - start)
        {
            if let Some((lo, hi, delta)) = entry.changed.take() {
                let text = d.rope().byte_slice(start..end);
                entry.wrapped = entry.wrapped.edited(text, lo, hi, delta, width, tab);
            }
            return entry.wrapped.rows.clone();
        }
        let wrapped = Wrapped::new(&d.slice(start, end), width, tab);
        let rows = wrapped.rows.clone();
        let entries = self.docs.values().flat_map(|l| l.entries.values());
        let (count, total) = entries.fold((0, 0), |(n, total), e| {
            (n + 1, total + e.wrapped.rows.len())
        });
        if count >= ENTRIES || total + rows.len() > ROWS {
            for lines in self.docs.values_mut() {
                lines.entries.clear();
            }
        }
        let lines = self.docs.entry(doc).or_default();
        lines.entries.insert(
            key,
            Entry {
                start,
                len: end - start,
                wrapped,
                changed: None,
            },
        );
        rows
    }
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

    /// Wrapping as it was before rows were resumable, for comparison.
    fn reference(line: &str, width: usize, tab: usize) -> Vec<Row> {
        let width = width.max(1);
        let tab = tab.max(1);
        let mut rows = vec![];
        let mut row = Row {
            start: 0,
            end: line.len(),
            col: 0,
        };
        let mut x = 0;
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

    fn xorshift(mut seed: u64) -> impl FnMut(usize) -> usize {
        move |n| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n.max(1) as u64) as usize
        }
    }

    #[test]
    fn rewrapped_ascii_windows_of_non_ascii_lines_break_at_the_same_spaces() {
        // The edit is far from the only non-ASCII character, so the window
        // re-wrapped around it is ASCII while the whole line is not.
        let line = format!("é {}", "abc\u{c}defgh\u{b}ij ".repeat(200));
        let mut d = Document::from_text(line).unwrap();
        let mut cache = Cache::default();
        cache.rows(1, &d, 0, 5, 4);
        let at = d.len() - 40;
        d.replace(at, at, "xy", 0).unwrap();
        assert_eq!(*cache.rows(1, &d, 0, 5, 4), reference(&d.text(), 5, 4));
    }

    #[test]
    fn cached_rows_follow_edits_and_match_a_fresh_wrap() {
        let pieces = [
            "a",
            "bc",
            " ",
            "\t",
            "猫",
            "é",
            "e\u{301}",
            "\u{301}",
            "👍",
            "\u{1f3fd}",
            "  ",
            "word ",
            "\u{b}",
            "\u{c}",
            "\n",
        ];
        let mut next = xorshift(0x9e37_79b9_7f4a_7c15);
        let mut cache = Cache::default();
        let mut d = Document::from_text("start of the text\n\tsecond line".into()).unwrap();
        for round in 0..1500 {
            let len = d.len();
            let a = d.floor_boundary(next(len + 1));
            let b = d.floor_boundary((a + next(8)).min(len)).max(a);
            let text: String = (0..next(6))
                .map(|_| pieces[next(pieces.len() - usize::from(round % 9 != 0))])
                .collect();
            d.replace(a, b, &text, 0).unwrap();
            for line in 0..d.line_count() {
                for (width, tab) in [(7, 4), (3, 3), (1, 2)] {
                    let (start, end) = d.line_range(line);
                    assert_eq!(
                        *cache.rows(1, &d, line, width, tab),
                        reference(&d.slice(start, end), width, tab),
                        "round {round} line {line} width {width}"
                    );
                }
            }
        }
    }

    #[test]
    fn edits_reuse_rows_of_other_lines_and_rewrap_long_lines_identically() {
        let mut d = Document::from_text(format!(
            "first line here\n{}\nlast line",
            "ab\tcd 猫 efg ".repeat(20_000)
        ))
        .unwrap();
        let mut cache = Cache::default();
        let first = cache.rows(1, &d, 0, 10, 4);
        let last = cache.rows(1, &d, 2, 10, 4);
        cache.rows(1, &d, 1, 10, 4);
        let mut next = xorshift(42);
        for _ in 0..20 {
            // Edits in the long line, near its start, middle or end.
            let (start, end) = d.line_range(1);
            let at = d.floor_boundary(start + next(end - start));
            let text = ["x", "\t", "", "猫 ", "  "][next(5)];
            let removed = d.floor_boundary((at + next(4)).min(end));
            d.replace(at, removed, text, 0).unwrap();
            let (start, end) = d.line_range(1);
            assert_eq!(
                *cache.rows(1, &d, 1, 10, 4),
                reference(&d.slice(start, end), 10, 4)
            );
        }
        let long = cache.rows(1, &d, 1, 10, 4);
        assert!(Arc::ptr_eq(&first, &cache.rows(1, &d, 0, 10, 4)));
        assert!(Arc::ptr_eq(&last, &cache.rows(1, &d, 2, 10, 4)));
        // A new line above shifts the others without re-wrapping them.
        d.replace(15, 15, "\nnew", 0).unwrap();
        assert!(Arc::ptr_eq(&long, &cache.rows(1, &d, 2, 10, 4)));
        assert!(Arc::ptr_eq(&last, &cache.rows(1, &d, 3, 10, 4)));
        // Editing a line re-wraps it; joining lines drops both.
        d.replace(4, 4, "a much longer first line", 0).unwrap();
        let (_, end) = d.line_range(0);
        assert_eq!(
            *cache.rows(1, &d, 0, 10, 4),
            reference(&d.slice(0, end), 10, 4)
        );
        assert!(!Arc::ptr_eq(&first, &cache.rows(1, &d, 0, 10, 4)));
        let (start, _) = d.line_range(3);
        d.replace(start - 1, start, "", 0).unwrap();
        let (start, end) = d.line_range(2);
        assert_eq!(
            *cache.rows(1, &d, 2, 10, 4),
            reference(&d.slice(start, end), 10, 4)
        );
    }
}
