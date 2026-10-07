//! Language-agnostic editing helpers: bracket and quote pairing, indentation
//! on Enter, bracket matching and indentation-based folding. They work on
//! the rope directly and look at a bounded neighbourhood, so they stay cheap
//! in very large documents.
use ropey::Rope;

/// Characters scanned when looking for a matching bracket.
const MATCH_LIMIT: usize = 200_000;
/// Lines scanned when deciding whether a line starts a fold.
const FOLD_PROBE: usize = 2_000;

pub fn closer(open: char) -> Option<char> {
    Some(match open {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        '"' => '"',
        '\'' => '\'',
        '`' => '`',
        _ => return None,
    })
}
fn opener(close: char) -> Option<char> {
    Some(match close {
        ')' => '(',
        ']' => '[',
        '}' => '{',
        _ => return None,
    })
}
fn is_quote(c: char) -> bool {
    matches!(c, '"' | '\'' | '`')
}

/// One replacement and where the caret (and selection anchor) end up,
/// relative to `start`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Typed {
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub caret: usize,
    pub anchor: Option<usize>,
}
impl Typed {
    pub fn plain(start: usize, end: usize, text: &str) -> Self {
        Self {
            start,
            end,
            text: text.into(),
            caret: text.len(),
            anchor: None,
        }
    }
}

fn char_before(rope: &Rope, offset: usize) -> Option<char> {
    let index = rope.byte_to_char(offset);
    (index > 0).then(|| rope.char(index - 1))
}
fn char_at(rope: &Rope, offset: usize) -> Option<char> {
    let index = rope.byte_to_char(offset);
    (index < rope.len_chars()).then(|| rope.char(index))
}

/// Typing `text` over `start..end`, pairing brackets and quotes when
/// `pairs` is on: an opener inserts its closer, a closer steps over the one
/// already there, and an opener typed over a selection wraps it.
pub fn type_text(rope: &Rope, start: usize, end: usize, text: &str, pairs: bool) -> Typed {
    let mut chars = text.chars();
    let (Some(c), None, true) = (chars.next(), chars.next(), pairs) else {
        return Typed::plain(start, end, text);
    };
    let next = char_at(rope, end);
    let previous = char_before(rope, start);
    if start != end {
        if let Some(close) = closer(c) {
            let selected = rope.byte_slice(start..end).to_string();
            if selected.contains('\n') && is_quote(c) {
                return Typed::plain(start, end, text);
            }
            let inner = c.len_utf8();
            return Typed {
                start,
                end,
                text: format!("{c}{selected}{close}"),
                caret: inner + selected.len(),
                anchor: Some(inner),
            };
        }
        return Typed::plain(start, end, text);
    }
    // Step over a closer (or closing quote) that is already there.
    if (opener(c).is_some() || is_quote(c)) && next == Some(c) {
        return Typed {
            start,
            end,
            text: String::new(),
            caret: c.len_utf8(),
            anchor: None,
        };
    }
    let Some(close) = closer(c) else {
        return Typed::plain(start, end, text);
    };
    let free_after = next.is_none_or(|n| {
        n.is_whitespace() || opener(n).is_some() || matches!(n, ',' | ';' | ':' | '.')
    });
    let quote_ok = !is_quote(c)
        || (previous.is_none_or(|p| !p.is_alphanumeric() && p != c && p != '\\')
            && next.is_none_or(|n| !n.is_alphanumeric()));
    if free_after && quote_ok {
        return Typed {
            start,
            end,
            text: format!("{c}{close}"),
            caret: c.len_utf8(),
            anchor: None,
        };
    }
    Typed::plain(start, end, text)
}

/// Backspace at an empty caret between a freshly paired opener and closer
/// deletes both.
pub fn pair_around(rope: &Rope, caret: usize) -> Option<(usize, usize)> {
    let before = char_before(rope, caret)?;
    let after = char_at(rope, caret)?;
    (closer(before) == Some(after)).then(|| (caret - before.len_utf8(), caret + after.len_utf8()))
}

fn leading_whitespace(rope: &Rope, line: usize) -> String {
    rope.line(line)
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

/// Enter over `start..end`: keep the line's indentation, indent one level
/// more after an opening bracket or a trailing colon, and put a closing
/// bracket right after the caret on its own line.
pub fn enter(rope: &Rope, start: usize, end: usize, unit: &str, auto_indent: bool) -> Typed {
    if !auto_indent {
        return Typed::plain(start, end, "\n");
    }
    let line = rope.byte_to_line(start);
    let line_start = rope.line_to_byte(line);
    let leading: String = leading_whitespace(rope, line);
    // Only the indentation before the caret counts when splitting it.
    let leading = &leading[..leading.len().min(start - line_start)];
    let before = rope.byte_slice(line_start..start).to_string();
    let last = before.trim_end().chars().last();
    let next = char_at(rope, end);
    let opens = last.is_some_and(|c| opener(c).is_none() && closer(c).is_some() && !is_quote(c));
    let deeper = opens || last == Some(':');
    if opens && next.is_some_and(|n| Some(n) == last.and_then(closer)) {
        let inner = format!("\n{leading}{unit}");
        return Typed {
            start,
            end,
            text: format!("{inner}\n{leading}"),
            caret: inner.len(),
            anchor: None,
        };
    }
    let text = format!("\n{leading}{}", if deeper { unit } else { "" });
    Typed::plain(start, end, &text)
}

/// Typing a closing bracket on a line holding only indentation re-indents
/// the line to match its opening bracket's line.
pub fn dedent_closer(rope: &Rope, caret: usize, c: char) -> Option<Typed> {
    let open = opener(c)?;
    let line = rope.byte_to_line(caret);
    let line_start = rope.line_to_byte(line);
    let before = rope.byte_slice(line_start..caret);
    if before.chars().any(|ch| ch != ' ' && ch != '\t') || before.len_bytes() == 0 {
        return None;
    }
    let target = unmatched_opener(rope, line_start, open, c)?;
    let indent = leading_whitespace(rope, rope.byte_to_line(target));
    if before == indent.as_str() {
        return None;
    }
    let text = format!("{indent}{c}");
    Some(Typed {
        start: line_start,
        end: caret,
        caret: text.len(),
        text,
        anchor: None,
    })
}

/// The nearest unmatched `open` before `offset`.
fn unmatched_opener(rope: &Rope, offset: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 0usize;
    let mut index = rope.byte_to_char(offset);
    let stop = index.saturating_sub(MATCH_LIMIT);
    while index > stop {
        index -= 1;
        let ch = rope.char(index);
        if ch == close {
            depth += 1;
        } else if ch == open {
            if depth == 0 {
                return Some(rope.char_to_byte(index));
            }
            depth -= 1;
        }
    }
    None
}

/// The bracket pair touching `offset` (the bracket after it, else before
/// it), as the byte offsets of both brackets.
pub fn matching_bracket(rope: &Rope, offset: usize) -> Option<(usize, usize)> {
    let candidates = [
        char_at(rope, offset).map(|c| (offset, c)),
        char_before(rope, offset).map(|c| (offset - c.len_utf8(), c)),
    ];
    for (at, c) in candidates.into_iter().flatten() {
        if let Some(close) = closer(c).filter(|_| !is_quote(c)) {
            let mut depth = 0usize;
            let first = rope.byte_to_char(at) + 1;
            let last = rope.len_chars().min(first + MATCH_LIMIT);
            for index in first..last {
                let ch = rope.char(index);
                if ch == c {
                    depth += 1;
                } else if ch == close {
                    if depth == 0 {
                        return Some((at, rope.char_to_byte(index)));
                    }
                    depth -= 1;
                }
            }
            return None;
        }
        if let Some(open) = opener(c) {
            return unmatched_opener(rope, at, open, c).map(|o| (o, at));
        }
    }
    None
}

fn indent_width(rope: &Rope, line: usize, tab: usize) -> Option<usize> {
    let mut width = 0;
    for c in rope.line(line).chars() {
        match c {
            ' ' => width += 1,
            '\t' => width += tab - width % tab.max(1),
            '\n' | '\r' => return None,
            _ => return Some(width),
        }
    }
    None
}

/// The last line folded under `line`: following lines indented deeper than
/// it (blank lines included, trailing ones excluded). `None` when nothing is
/// indented under it.
pub fn fold_end(rope: &Rope, line: usize, tab: usize) -> Option<usize> {
    let base = indent_width(rope, line, tab)?;
    let lines = rope.len_lines();
    let mut end = None;
    for next in line + 1..lines {
        match indent_width(rope, next, tab) {
            None => continue,
            Some(width) if width > base => end = Some(next),
            Some(_) => break,
        }
    }
    end
}

/// Whether `line` starts a fold, looking only at the next non-blank line.
pub fn foldable(rope: &Rope, line: usize, tab: usize) -> bool {
    let Some(base) = indent_width(rope, line, tab) else {
        return false;
    };
    (line + 1..rope.len_lines().min(line + 1 + FOLD_PROBE))
        .find_map(|next| indent_width(rope, next, tab))
        .is_some_and(|width| width > base)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(text: &str, t: &Typed) -> (String, usize) {
        let mut s = text.to_string();
        s.replace_range(t.start..t.end, &t.text);
        (s, t.start + t.caret)
    }

    #[test]
    fn brackets_and_quotes_pair_wrap_and_step_over() {
        let rope = Rope::from_str("f x");
        let t = type_text(&rope, 1, 1, "(", true);
        assert_eq!(apply("f x", &t), ("f() x".into(), 2));
        let rope = Rope::from_str("f()");
        let t = type_text(&rope, 2, 2, ")", true);
        assert_eq!(apply("f()", &t), ("f()".into(), 3), "steps over");
        // Before a word, or inside one, nothing is paired.
        let rope = Rope::from_str("word");
        assert_eq!(type_text(&rope, 0, 0, "(", true).text, "(");
        let rope = Rope::from_str("it");
        assert_eq!(type_text(&rope, 1, 1, "'", true).text, "'", "it's");
        // Wrapping keeps the selection.
        let rope = Rope::from_str("a sel b");
        let t = type_text(&rope, 2, 5, "[", true);
        assert_eq!(apply("a sel b", &t).0, "a [sel] b");
        assert_eq!((t.anchor, t.caret), (Some(1), 4));
        assert_eq!(type_text(&rope, 1, 1, "(", false).text, "(");
        let rope = Rope::from_str("f()");
        assert_eq!(pair_around(&rope, 2), Some((1, 3)));
        assert_eq!(pair_around(&rope, 1), None);
    }

    #[test]
    fn enter_indents_after_openers_and_splits_pairs() {
        let text = "    if x {}";
        let rope = Rope::from_str(text);
        let t = enter(&rope, 10, 10, "    ", true);
        assert_eq!(apply(text, &t), ("    if x {\n        \n    }".into(), 19));
        let text = "def f():";
        let t = enter(&Rope::from_str(text), 8, 8, "  ", true);
        assert_eq!(apply(text, &t).0, "def f():\n  ");
        let text = "  plain";
        let t = enter(&Rope::from_str(text), 7, 7, "  ", true);
        assert_eq!(apply(text, &t).0, "  plain\n  ");
        assert_eq!(enter(&Rope::from_str(text), 7, 7, "  ", false).text, "\n");
    }

    #[test]
    fn closers_dedent_to_their_opener() {
        let text = "fn f() {\n    x\n        ";
        let rope = Rope::from_str(text);
        let t = dedent_closer(&rope, text.len(), '}').unwrap();
        assert_eq!(apply(text, &t).0, "fn f() {\n    x\n}");
        assert!(
            dedent_closer(&rope, 9, '}').is_none(),
            "not on an indented line"
        );
    }

    #[test]
    fn brackets_match_in_both_directions_with_nesting() {
        let rope = Rope::from_str("a(b[c](d))e");
        assert_eq!(matching_bracket(&rope, 1), Some((1, 9)));
        assert_eq!(matching_bracket(&rope, 10), Some((1, 9)));
        assert_eq!(matching_bracket(&rope, 3), Some((3, 5)));
        assert_eq!(matching_bracket(&rope, 0), None);
        assert_eq!(matching_bracket(&Rope::from_str("(("), 0), None);
    }

    #[test]
    fn folds_follow_indentation() {
        let text = "fn a() {\n    one\n\n    two\n}\nfn b() {}\n";
        let rope = Rope::from_str(text);
        assert_eq!(fold_end(&rope, 0, 4), Some(3));
        assert!(foldable(&rope, 0, 4));
        assert!(!foldable(&rope, 1, 4));
        assert_eq!(fold_end(&rope, 5, 4), None);
        assert_eq!(fold_end(&rope, 2, 4), None, "blank lines do not fold");
    }
}
