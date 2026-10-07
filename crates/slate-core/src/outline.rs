//! Document symbols from the syntax grammar, for files without a language
//! server: functions, types, modules and headings that the grammar names.
use ropey::Rope;
use std::path::Path;
use syntect::parsing::{ParseState, Scope, ScopeStack};

/// Lines parsed for the outline; longer files list their first part.
const LINE_LIMIT: usize = 200_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub kind: String,
    /// Zero-based line and byte column.
    pub line: usize,
    pub column: usize,
}

fn kind_of(scope: &str) -> Option<&'static str> {
    const KINDS: &[(&str, &str)] = &[
        ("entity.name.function", "function"),
        ("entity.name.method", "method"),
        ("entity.name.class", "class"),
        ("entity.name.struct", "struct"),
        ("entity.name.enum", "enum"),
        ("entity.name.union", "union"),
        ("entity.name.trait", "trait"),
        ("entity.name.interface", "interface"),
        ("entity.name.impl", "impl"),
        ("entity.name.type", "type"),
        ("entity.name.module", "module"),
        ("entity.name.namespace", "namespace"),
        ("entity.name.macro", "macro"),
        ("entity.name.constant", "constant"),
        ("entity.name.section", "heading"),
        ("entity.name.tag.yaml", "key"),
    ];
    KINDS
        .iter()
        .find(|(prefix, _)| scope.starts_with(prefix))
        .map(|(_, kind)| *kind)
}

/// Symbols in document order.
pub fn symbols(rope: &Rope, path: Option<&Path>) -> Vec<Symbol> {
    let first = rope.line(0).to_string();
    let syntax = crate::highlight::syntax_for(path, &first);
    let syntaxes = crate::highlight::syntaxes();
    let mut state = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let heading = Scope::new("markup.heading").ok();
    let mut out = Vec::new();
    for (line_number, line) in rope.lines().take(LINE_LIMIT).enumerate() {
        let line = line.to_string();
        let Ok(ops) = state.parse_line(&line, syntaxes) else {
            break;
        };
        let mut position = 0;
        let mut current: Option<(usize, &'static str)> = None;
        let mut flush = |end: usize, current: &mut Option<(usize, &'static str)>| {
            if let Some((start, kind)) = current.take() {
                let name = line[start..end.min(line.len())].trim();
                if !name.is_empty() {
                    out.push(Symbol {
                        name: name.to_string(),
                        kind: kind.into(),
                        line: line_number,
                        column: start,
                    });
                }
            }
        };
        for (offset, op) in ops {
            if offset > position {
                let kind = stack
                    .as_slice()
                    .iter()
                    .rev()
                    .find_map(|s| kind_of(&s.build_string()));
                match (kind, current) {
                    (Some(k), None) => current = Some((position, k)),
                    (Some(k), Some((_, c))) if k != c => {
                        flush(position, &mut current);
                        current = Some((position, k));
                    }
                    (None, Some(_)) => flush(position, &mut current),
                    _ => {}
                }
                position = offset;
            }
            let _ = stack.apply(&op);
        }
        let kind = stack
            .as_slice()
            .iter()
            .rev()
            .find_map(|s| kind_of(&s.build_string()));
        match (kind, current) {
            (Some(k), None) => {
                current = Some((position, k));
                flush(line.len(), &mut current);
            }
            (Some(_), Some(_)) => flush(line.len(), &mut current),
            (None, Some(_)) => flush(position, &mut current),
            _ => {}
        }
        // Markdown headings name a whole line.
        if let Some(heading) = heading {
            if out.last().is_none_or(|s| s.line != line_number)
                && stack.as_slice().iter().any(|s| heading.is_prefix_of(*s))
                && line.starts_with('#')
            {
                out.push(Symbol {
                    name: line.trim_start_matches('#').trim().to_string(),
                    kind: "heading".into(),
                    line: line_number,
                    column: 0,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grammars_name_functions_and_types() {
        let rust = Rope::from_str(
            "struct Point { x: i32 }\nimpl Point {\n    fn new() -> Self { todo!() }\n}\nfn main() {}\n",
        );
        let found: Vec<(String, String, usize)> = symbols(&rust, Some(Path::new("a.rs")))
            .into_iter()
            .map(|s| (s.name, s.kind, s.line))
            .collect();
        assert!(
            found.contains(&("Point".into(), "struct".into(), 0)),
            "{found:?}"
        );
        assert!(
            found.iter().any(|(n, _, l)| n == "new" && *l == 2),
            "{found:?}"
        );
        assert!(found
            .iter()
            .any(|(n, k, l)| n == "main" && k == "function" && *l == 4));
        let python = Rope::from_str("class A:\n    def run(self):\n        pass\n");
        let found = symbols(&python, Some(Path::new("a.py")));
        assert_eq!(found[0].name, "A");
        assert_eq!(found[1].name, "run");
        let markdown = Rope::from_str("# Title\n\ntext\n## Part two\n");
        let found = symbols(&markdown, Some(Path::new("a.md")));
        let names: Vec<_> = found.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"Title") && names.contains(&"Part two"),
            "{names:?}"
        );
    }
}
