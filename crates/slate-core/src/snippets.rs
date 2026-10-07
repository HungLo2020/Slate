//! Snippets: a word followed by Tab expands into a template whose tab stops
//! (`$1`, `${2:default}`, final `$0`) Tab and Shift+Tab then visit. A stop
//! used more than once is edited at all its places at the same time.
//!
//! Built-in snippets cover common languages; `snippets.toml` in the config
//! directory adds more:
//!
//! ```toml
//! [[snippet]]
//! prefix = "todo"
//! body = "// TODO($1): $0"
//! language = "Rust"   # optional; a syntax name, or omitted for every file
//! ```
use crate::{cursors::Selection, App};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize)]
pub struct Snippet {
    pub prefix: String,
    pub body: String,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub description: String,
}

#[derive(Deserialize)]
struct File {
    #[serde(default)]
    snippet: Vec<Snippet>,
}

const BUILTIN: &[(&str, &str, &str)] = &[
    ("Rust", "fn", "fn ${1:name}($2)${3: -> ${4:()}} {\n\t$0\n}"),
    ("Rust", "test", "#[test]\nfn ${1:name}() {\n\t$0\n}"),
    ("Rust", "impl", "impl ${1:Type} {\n\t$0\n}"),
    ("Rust", "match", "match ${1:value} {\n\t${2:_} => $0,\n}"),
    ("Rust", "for", "for ${1:item} in ${2:items} {\n\t$0\n}"),
    ("Rust", "if", "if ${1:condition} {\n\t$0\n}"),
    ("Python", "def", "def ${1:name}(${2}):\n\t${0:pass}"),
    (
        "Python",
        "class",
        "class ${1:Name}:\n\tdef __init__(self${2}):\n\t\t${0:pass}",
    ),
    ("Python", "for", "for ${1:item} in ${2:items}:\n\t${0:pass}"),
    ("Python", "if", "if ${1:condition}:\n\t${0:pass}"),
    (
        "Python",
        "main",
        "if __name__ == \"__main__\":\n\t${0:main()}",
    ),
    ("JavaScript", "fn", "function ${1:name}($2) {\n\t$0\n}"),
    (
        "JavaScript",
        "for",
        "for (const ${1:item} of ${2:items}) {\n\t$0\n}",
    ),
    ("JavaScript", "log", "console.log($0);"),
    (
        "TypeScript",
        "fn",
        "function ${1:name}($2): ${3:void} {\n\t$0\n}",
    ),
    (
        "TypeScript",
        "for",
        "for (const ${1:item} of ${2:items}) {\n\t$0\n}",
    ),
    (
        "C",
        "main",
        "int main(int argc, char **argv) {\n\t$0\n\treturn 0;\n}",
    ),
    (
        "C",
        "for",
        "for (${1:int} ${2:i} = 0; $2 < ${3:n}; $2++) {\n\t$0\n}",
    ),
    ("C", "inc", "#include <${1:stdio.h}>"),
    (
        "C++",
        "main",
        "int main(int argc, char **argv) {\n\t$0\n\treturn 0;\n}",
    ),
    (
        "C++",
        "for",
        "for (${1:int} ${2:i} = 0; $2 < ${3:n}; ++$2) {\n\t$0\n}",
    ),
    ("Go", "fn", "func ${1:name}($2) ${3:error} {\n\t$0\n}"),
    ("Go", "iferr", "if err != nil {\n\treturn ${1:err}\n}"),
    (
        "Bourne Again Shell (bash)",
        "if",
        "if [[ ${1:condition} ]]; then\n\t$0\nfi",
    ),
    (
        "Bourne Again Shell (bash)",
        "for",
        "for ${1:item} in ${2:items}; do\n\t$0\ndone",
    ),
];

/// Text with tab stops: `stops[n]` lists the byte ranges of stop `n`.
#[derive(Debug, PartialEq, Eq)]
pub struct Expansion {
    pub text: String,
    pub stops: BTreeMap<usize, Vec<(usize, usize)>>,
}

/// Expand a snippet body. `indent` follows every line break; a tab in the
/// body becomes one indentation `unit`.
pub fn expand(body: &str, indent: &str, unit: &str) -> Expansion {
    let chars: Vec<char> = body.chars().collect();
    // The first pass learns each stop's default text, so the second can
    // repeat it at bare mirrors (`$1` after `${1:x}`).
    let mut defaults = BTreeMap::new();
    for _ in 0..2 {
        let mut text = String::new();
        let mut stops: BTreeMap<usize, Vec<(usize, usize)>> = BTreeMap::new();
        let mut parser = Parser {
            chars: &chars,
            i: 0,
            out: &mut text,
            stops: &mut stops,
            indent,
            unit,
            defaults: &defaults,
        };
        parser.parse(false);
        let learned: BTreeMap<usize, String> = stops
            .iter()
            .filter_map(|(n, ranges)| {
                let (a, b) = ranges.iter().find(|(a, b)| a < b)?;
                Some((*n, text[*a..*b].to_string()))
            })
            .collect();
        if learned == defaults {
            return Expansion { text, stops };
        }
        defaults = learned;
    }
    let mut text = String::new();
    let mut stops = BTreeMap::new();
    Parser {
        chars: &chars,
        i: 0,
        out: &mut text,
        stops: &mut stops,
        indent,
        unit,
        defaults: &defaults,
    }
    .parse(false);
    Expansion { text, stops }
}

struct Parser<'a> {
    chars: &'a [char],
    i: usize,
    out: &'a mut String,
    stops: &'a mut BTreeMap<usize, Vec<(usize, usize)>>,
    indent: &'a str,
    unit: &'a str,
    defaults: &'a BTreeMap<usize, String>,
}
impl Parser<'_> {
    fn parse(&mut self, nested: bool) {
        while self.i < self.chars.len() {
            let c = self.chars[self.i];
            match c {
                '\\' if self.i + 1 < self.chars.len() => {
                    self.out.push(self.chars[self.i + 1]);
                    self.i += 2;
                }
                '}' if nested => return,
                '\n' => {
                    self.out.push('\n');
                    self.out.push_str(self.indent);
                    self.i += 1;
                }
                '\t' => {
                    self.out.push_str(self.unit);
                    self.i += 1;
                }
                '$' => self.stop(),
                _ => {
                    self.out.push(c);
                    self.i += 1;
                }
            }
        }
    }
    fn stop(&mut self) {
        self.i += 1;
        let braced = self.chars.get(self.i) == Some(&'{');
        if braced {
            self.i += 1;
        }
        let digits: String = self.chars[self.i..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if digits.is_empty() {
            self.out.push('$');
            if braced {
                self.out.push('{');
            }
            return;
        }
        self.i += digits.len();
        let number: usize = digits.parse().unwrap_or(0);
        let start = self.out.len();
        let mut placeholder = false;
        if braced {
            if self.chars.get(self.i) == Some(&':') {
                self.i += 1;
                placeholder = true;
                self.parse(true);
            }
            if self.chars.get(self.i) == Some(&'}') {
                self.i += 1;
            }
        }
        if !placeholder {
            if let Some(default) = self.defaults.get(&number) {
                self.out.push_str(default);
            }
        }
        self.stops
            .entry(number)
            .or_default()
            .push((start, self.out.len()));
    }
}

/// Snippets for a syntax: user ones first, then built-in ones.
pub fn available(language: &str) -> Vec<Snippet> {
    let path = crate::paths::config_dir().join("snippets.toml");
    let mut list: Vec<Snippet> = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| toml::from_str::<File>(&s).ok())
        .map(|f| f.snippet)
        .unwrap_or_default();
    list.extend(BUILTIN.iter().map(|(language, prefix, body)| Snippet {
        prefix: prefix.to_string(),
        body: body.to_string(),
        language: Some(language.to_string()),
        description: String::new(),
    }));
    list.retain(|s| s.language.as_deref().is_none_or(|l| l == language));
    list
}

impl App {
    fn language_of(&self, doc: u64) -> String {
        let d = &self.documents[&doc];
        crate::highlight::syntax_for(d.path.as_deref(), &d.line_text(0))
            .name
            .clone()
    }

    /// Replace `start..end` with an expanded snippet and visit its first
    /// field.
    pub(crate) fn insert_expansion(
        &mut self,
        id: u64,
        start: usize,
        end: usize,
        expansion: Expansion,
    ) -> anyhow::Result<()> {
        let cursor = self.views[&id].cursor;
        self.edit_range(id, start, end, &expansion.text, cursor)?;
        let place = |ranges: &Vec<(usize, usize)>| -> Vec<(usize, usize)> {
            ranges.iter().map(|(a, b)| (start + a, start + b)).collect()
        };
        let mut stops: Vec<Vec<(usize, usize)>> = expansion
            .stops
            .iter()
            .filter(|(n, _)| **n > 0)
            .map(|(_, r)| place(r))
            .collect();
        let end = start + expansion.text.len();
        stops.push(
            expansion
                .stops
                .get(&0)
                .map(&place)
                .unwrap_or_else(|| vec![(end, end)]),
        );
        let v = self.views.get_mut(&id).unwrap();
        v.stops = stops;
        v.visited.clear();
        v.current_stop.clear();
        self.snippet_tab(id, false)?;
        Ok(())
    }

    /// Tab: visit the next tab stop of an active snippet, or expand the word
    /// before the caret. Returns false when there is nothing to do.
    pub(crate) fn snippet_tab(&mut self, id: u64, backwards: bool) -> anyhow::Result<bool> {
        if !self.views[&id].stops.is_empty() {
            let v = self.views.get_mut(&id).unwrap();
            let next = if backwards {
                // Shift+Tab returns to the previously visited stop.
                match v.visited.pop() {
                    Some(previous) => {
                        let current = std::mem::take(&mut v.current_stop);
                        v.stops.insert(0, current);
                        previous
                    }
                    None => return Ok(true),
                }
            } else {
                let next = v.stops.remove(0);
                let current = std::mem::take(&mut v.current_stop);
                if !current.is_empty() {
                    v.visited.push(current);
                }
                next
            };
            v.current_stop = next.clone();
            let selections = next
                .iter()
                .map(|(a, b)| Selection {
                    cursor: *b,
                    anchor: (a != b).then_some(*a),
                })
                .collect();
            self.set_selections(id, selections);
            if self.views[&id].stops.is_empty() {
                let v = self.views.get_mut(&id).unwrap();
                v.visited.clear();
                v.current_stop.clear();
            }
            return Ok(true);
        }
        if backwards {
            return Ok(false);
        }
        let v = &self.views[&id];
        if v.anchor.is_some() || !v.extra.is_empty() {
            return Ok(false);
        }
        let doc = &self.documents[&v.document];
        let line = doc.line_of(v.cursor);
        let (start, _) = doc.line_range(line);
        let before = doc.slice(start, v.cursor).into_owned();
        let word: String = before
            .chars()
            .rev()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        if word.is_empty() {
            return Ok(false);
        }
        let Some(snippet) = available(&self.language_of(v.document))
            .into_iter()
            .find(|s| s.prefix == word)
        else {
            return Ok(false);
        };
        let indent: String = before
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        let expansion = expand(&snippet.body, &indent, &self.indent_unit());
        let word_start = v.cursor - word.len();
        let cursor = v.cursor;
        self.insert_expansion(id, word_start, cursor, expansion)?;
        self.status = format!("Snippet {} · Tab: next field", snippet.prefix);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bodies_expand_with_stops_defaults_mirrors_and_indentation() {
        let e = expand("for ${1:i} in ${2:xs} {\n\t$1$0\n}", "  ", "    ");
        assert_eq!(e.text, "for i in xs {\n      i\n  }");
        assert_eq!(e.stops[&1], vec![(4, 5), (20, 21)]);
        assert_eq!(e.stops[&2], vec![(9, 11)]);
        assert_eq!(e.stops[&0], vec![(21, 21)]);
        let nested = expand("${1:a ${2:b}} \\$x", "", "\t");
        assert_eq!(nested.text, "a b $x");
        assert_eq!(nested.stops[&1], vec![(0, 3)]);
        assert_eq!(nested.stops[&2], vec![(2, 3)]);
    }

    #[test]
    fn builtin_snippets_are_well_formed() {
        for (_, prefix, body) in BUILTIN {
            let e = expand(body, "", "    ");
            assert!(!e.text.contains('$'), "{prefix}: {}", e.text);
            assert!(!e.stops.is_empty(), "{prefix}");
        }
    }
}
