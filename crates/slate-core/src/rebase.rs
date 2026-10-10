//! Moving positions across a batch of replacements at once.
//!
//! The replacements are processed from the last in the text to the first,
//! all in the coordinates of the text before them, and a position follows
//! each in turn. `Batch` gives the same results in O(log n) per position:
//! once a position lies beyond the next replacement down, every remaining
//! one only shifts it, which is a prefix sum. A position that lands exactly
//! on a replacement's start continues from a result kept for that start.

/// One replacement: `start..end` of the original text became `len` bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Edit {
    pub start: usize,
    pub end: usize,
    pub len: usize,
    /// For inserted text containing a line break: where its last line
    /// starts. Line marks at the insertion point follow that line.
    pub pushed: Option<usize>,
}
impl Edit {
    pub(crate) fn text(start: usize, end: usize, text: &str) -> Self {
        Self {
            start,
            end,
            len: text.len(),
            pushed: text.rfind('\n').map(|i| start + i + 1),
        }
    }
}

/// How a position at (or inside) a replacement moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Stays before text replacing or inserted at it.
    Before,
    /// Moves after text replacing or inserted at it (a field growing as
    /// text is typed at its end).
    After,
    /// Moves after text inserted at it, but stays before a replacement.
    AfterInsertion,
    /// A mark on a line start: follows its line when text with a line break
    /// is inserted there, and vanishes when the start is replaced.
    Line,
    /// `Line`, but a replaced mark moves to the replacement's start.
    Breakpoint,
}
const MODES: usize = 5;

/// One replacement's effect on a position in `mode`; `None` drops it.
fn step(e: &Edit, mode: Mode, p: usize) -> Option<usize> {
    let position = |p: usize| {
        if p <= e.start {
            p
        } else if p >= e.end {
            p - e.end + e.start + e.len
        } else {
            e.start + e.len
        }
    };
    let insertion = e.start == e.end;
    Some(match mode {
        Mode::Before => position(p),
        Mode::After if p == e.start => e.start + e.len,
        Mode::AfterInsertion if insertion && p == e.start => p + e.len,
        Mode::After | Mode::AfterInsertion => position(p),
        Mode::Line | Mode::Breakpoint => {
            if insertion && p == e.start {
                e.pushed.unwrap_or(p)
            } else if e.start <= p && p < e.end {
                if mode == Mode::Line {
                    return None;
                }
                e.start
            } else {
                position(p)
            }
        }
    })
}

pub(crate) struct Batch {
    /// Ascending; processing runs from the last to the first.
    edits: Vec<Edit>,
    /// `shift[i]`: the size change of `edits[..i]`.
    shift: Vec<isize>,
    /// Per mode, per edit: where a position at that edit's start ends up
    /// once the edits before it are processed (filled on demand).
    #[allow(clippy::type_complexity)]
    collapsed: [Vec<Option<Option<usize>>>; MODES],
}
impl Batch {
    /// Edits in the order they are processed: each must lie at or before
    /// the previous one without overlapping it, so all share the original
    /// coordinates. `None` otherwise.
    pub(crate) fn new(processed: impl IntoIterator<Item = Edit>) -> Option<Self> {
        let mut edits: Vec<Edit> = processed.into_iter().collect();
        if edits.iter().any(|e| e.start > e.end)
            || edits.windows(2).any(|pair| pair[1].end > pair[0].start)
        {
            return None;
        }
        edits.reverse();
        let mut shift = Vec::with_capacity(edits.len() + 1);
        shift.push(0);
        for e in &edits {
            let last = *shift.last().unwrap();
            shift.push(last + e.len as isize - (e.end - e.start) as isize);
        }
        let collapsed = std::array::from_fn(|_| Vec::new());
        Some(Self {
            edits,
            shift,
            collapsed,
        })
    }
    /// Edits processed from the first in the text to the last, each in the
    /// coordinates the ones before it left (as undo runs a group). When no
    /// two touch, processing them from the last instead moves every
    /// position the same way, so they become one batch. `None` otherwise.
    pub(crate) fn ascending(processed: &[Edit]) -> Option<Self> {
        let mut common: Vec<Edit> = Vec::with_capacity(processed.len());
        let mut shift = 0isize;
        for e in processed {
            let start = e.start.checked_add_signed(-shift)?;
            let end = start + e.end.checked_sub(e.start)?;
            if common.last().is_some_and(|before| before.end >= start) {
                return None;
            }
            shift += e.len as isize - (e.end - e.start) as isize;
            common.push(Edit {
                start,
                end,
                len: e.len,
                pushed: e.pushed.map(|p| p - e.start + start),
            });
        }
        common.reverse();
        Self::new(common)
    }
    /// `new` for replacements given in any order, processed from the last
    /// in the text (ties in the order given). They must not overlap.
    pub(crate) fn sorted(edits: impl IntoIterator<Item = Edit>) -> Option<Self> {
        let mut edits: Vec<Edit> = edits.into_iter().collect();
        edits.sort_by_key(|e| std::cmp::Reverse((e.start, e.end)));
        Self::new(edits)
    }
    pub(crate) fn position(&mut self, p: usize) -> usize {
        self.map(Mode::Before, p).unwrap_or(p)
    }
    pub(crate) fn map(&mut self, mode: Mode, p: usize) -> Option<usize> {
        // Edits starting after `p` leave it alone in every mode.
        let i = self.edits.partition_point(|e| e.start <= p);
        let Some(j) = i.checked_sub(1) else {
            return Some(p);
        };
        let e = self.edits[j];
        if p > e.end {
            return Some(p.saturating_add_signed(self.shift[i]));
        }
        let q = step(&e, mode, p)?;
        if q == e.start {
            self.collapse(mode, j)
        } else {
            // Beyond the next edit down: only shifts remain.
            Some(q.saturating_add_signed(self.shift[j]))
        }
    }
    /// Where a position at the start of edit `j` ends up after the edits
    /// before it.
    fn collapse(&mut self, mode: Mode, j: usize) -> Option<usize> {
        let memo = &mut self.collapsed[mode as usize];
        if memo.is_empty() {
            memo.resize(self.edits.len(), None);
        }
        let mut chain = vec![];
        let mut k = j;
        let result = loop {
            if let Some(known) = memo[k] {
                break known;
            }
            chain.push(k);
            let q = self.edits[k].start;
            let Some(previous) = k.checked_sub(1) else {
                break Some(q);
            };
            let e = self.edits[previous];
            if q > e.end {
                break Some(q.saturating_add_signed(self.shift[k]));
            }
            match step(&e, mode, q) {
                None => break None,
                Some(r) if r == e.start => k = previous,
                Some(r) => break Some(r.saturating_add_signed(self.shift[previous])),
            }
        };
        for k in chain {
            memo[k] = Some(result);
        }
        result
    }
    /// A snippet field: its start moves after text inserted there, and an
    /// empty field's end goes with it.
    pub(crate) fn field(&mut self, a: usize, b: usize) -> (usize, usize) {
        // Edits starting after the field's end leave it alone.
        let mut i = self.edits.partition_point(|e| e.start <= b);
        let (mut a, mut b) = (a, b);
        while let Some(j) = i.checked_sub(1) {
            let e = self.edits[j];
            if a > e.end {
                let shift = self.shift[i];
                return (
                    a.saturating_add_signed(shift),
                    b.saturating_add_signed(shift),
                );
            }
            let follows = e.start == e.end && a == e.start;
            b = if follows {
                b + e.len
            } else {
                step(&e, Mode::Before, b).unwrap_or(b)
            };
            a = step(&e, Mode::AfterInsertion, a).unwrap_or(a);
            i = j;
        }
        (a, b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The per-edit rules `rebase_views` applied to each edit in turn before
    /// batching, transcribed independently of `step`.
    fn one_edit(e: &Edit, mode: Mode, p: usize) -> Option<usize> {
        let (start, end, length) = (e.start, e.end, e.len);
        let position = |p: usize| {
            if p <= start {
                p
            } else if p >= end {
                p - end + start + length
            } else {
                start + length
            }
        };
        let insertion = start == end;
        let line_mark = |p: usize| -> Option<usize> {
            if insertion && p == start {
                Some(e.pushed.unwrap_or(p))
            } else if start <= p && p < end {
                None
            } else {
                Some(position(p))
            }
        };
        match mode {
            Mode::Before => Some(position(p)),
            // A field end with its start at or before the edit (always so
            // for the fields checked below).
            Mode::After => Some(if p == start {
                start + length
            } else {
                position(p)
            }),
            Mode::AfterInsertion => Some(if insertion && p == start {
                p + length
            } else {
                position(p)
            }),
            Mode::Line => line_mark(p),
            Mode::Breakpoint => Some(line_mark(p).unwrap_or(start)),
        }
    }
    /// The reference: each edit in processing order, one at a time.
    fn sequential(edits: &[Edit], mode: Mode, p: usize) -> Option<usize> {
        edits.iter().try_fold(p, |p, e| one_edit(e, mode, p))
    }
    fn sequential_current(edits: &[Edit], a: usize, b: usize) -> (usize, usize) {
        edits.iter().fold((a, b), |(a, b), e| {
            let grows = b == e.start && a <= e.start;
            let a2 = one_edit(e, Mode::Before, a).unwrap();
            let b2 = if grows {
                e.start + e.len
            } else {
                one_edit(e, Mode::Before, b).unwrap()
            };
            (a2, b2)
        })
    }
    fn sequential_field(edits: &[Edit], a: usize, b: usize) -> (usize, usize) {
        edits.iter().fold((a, b), |(a, b), e| {
            let follows = e.start == e.end && a == e.start;
            let b = if follows {
                b + e.len
            } else {
                one_edit(e, Mode::Before, b).unwrap()
            };
            let a = if follows {
                a + e.len
            } else {
                one_edit(e, Mode::Before, a).unwrap()
            };
            (a, b)
        })
    }

    #[test]
    fn batched_positions_match_one_edit_at_a_time() {
        let mut seed = 0x0123_4567_89ab_cdefu64;
        let mut next = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        for round in 0..2000 {
            // Dense, touching edits: insertions, deletions and replacements
            // sharing boundaries, several insertions at one point.
            let mut edits = vec![];
            let mut at = next(3);
            for _ in 0..1 + next(12) {
                let start = at;
                let end = start + [0, 0, 1, 2, 3][next(5)];
                let len = [0, 0, 1, 3][next(4)];
                let pushed = (len > 0 && next(2) == 0).then(|| start + next(len + 1));
                edits.push(Edit {
                    start,
                    end,
                    len,
                    pushed,
                });
                at = end + [0, 0, 0, 1, 2][next(5)];
            }
            // Ties keep the given order, as `sort_by_key` does.
            let mut processed = edits.clone();
            processed.sort_by_key(|e| std::cmp::Reverse((e.start, e.end)));
            let mut batch = Batch::sorted(edits).unwrap();
            for mode in [
                Mode::Before,
                Mode::After,
                Mode::AfterInsertion,
                Mode::Line,
                Mode::Breakpoint,
            ] {
                for p in 0..at + 3 {
                    assert_eq!(
                        batch.map(mode, p),
                        sequential(&processed, mode, p),
                        "round {round} {mode:?} at {p}: {processed:?}"
                    );
                }
            }
            for a in 0..at + 2 {
                for b in a..at + 3 {
                    assert_eq!(
                        batch.field(a, b),
                        sequential_field(&processed, a, b),
                        "round {round} field {a}..{b}: {processed:?}"
                    );
                    let current = (batch.position(a), batch.map(Mode::After, b).unwrap());
                    assert_eq!(
                        current,
                        sequential_current(&processed, a, b),
                        "round {round} current field {a}..{b}: {processed:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn separated_edits_processed_first_to_last_batch_the_same() {
        let mut seed = 0x5851_f42d_4c95_7f2du64;
        let mut next = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        for round in 0..2000 {
            // Each edit in the coordinates left by the ones before it.
            let mut processed = vec![];
            let mut at = next(3);
            for _ in 0..1 + next(8) {
                let (start, end) = (at, at + [0, 1, 2][next(3)]);
                let len = [0, 1, 3][next(3)];
                processed.push(Edit {
                    start,
                    end,
                    len,
                    pushed: None,
                });
                // Usually apart; touching ones refuse to batch.
                at = start + len + [1, 2, 0][next(3)];
            }
            let Some(mut batch) = Batch::ascending(&processed) else {
                assert!(processed
                    .windows(2)
                    .any(|pair| pair[1].start == pair[0].start + pair[0].len));
                continue;
            };
            let total: usize = processed.iter().map(|e| e.end - e.start).sum::<usize>() + at;
            for mode in [
                Mode::Before,
                Mode::After,
                Mode::AfterInsertion,
                Mode::Line,
                Mode::Breakpoint,
            ] {
                for p in 0..total {
                    assert_eq!(
                        batch.map(mode, p),
                        sequential(&processed, mode, p),
                        "round {round} {mode:?} at {p}: {processed:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn edits_out_of_processing_order_are_refused() {
        let at = |start, end| Edit {
            start,
            end,
            len: 1,
            pushed: None,
        };
        assert!(Batch::new([at(5, 6), at(1, 2)]).is_some());
        assert!(Batch::new([at(1, 2), at(5, 6)]).is_none());
        assert!(Batch::new([at(3, 6), at(1, 4)]).is_none());
        assert!(Batch::new([at(3, 3), at(3, 3), at(1, 3)]).is_some());
        assert!(Batch::new([at(3, 3), at(3, 5)]).is_none());
    }
}
