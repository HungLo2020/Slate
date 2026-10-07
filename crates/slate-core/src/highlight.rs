use serde::Serialize;
use std::{path::Path, sync::OnceLock};
use syntect::{
    easy::HighlightLines,
    highlighting::{FontStyle, ThemeSet},
    parsing::SyntaxSet,
    util::LinesWithEndings,
};
#[derive(Clone, Serialize)]
pub struct Token {
    pub start: usize,
    pub end: usize,
    pub fg: String,
    pub bold: bool,
    pub italic: bool,
}
pub fn highlight(text: &str, path: Option<&Path>, light: bool) -> Vec<Token> {
    static SYNTAX: OnceLock<SyntaxSet> = OnceLock::new();
    static THEMES: OnceLock<ThemeSet> = OnceLock::new();
    // bat's grammar collection: TOML, TypeScript, Dockerfile, Nix, … beyond
    // syntect's built-in set.
    let ss = SYNTAX.get_or_init(two_face::syntax::extra_newlines);
    let ts = THEMES.get_or_init(ThemeSet::load_defaults);
    let syntax = path
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .and_then(|name| {
            // Whole names first (Makefile, Dockerfile, .bashrc), then extensions.
            ss.find_syntax_by_extension(name)
                .or_else(|| ss.find_syntax_by_extension(name.rsplit('.').next().unwrap_or(name)))
        })
        .or_else(|| {
            text.lines()
                .next()
                .and_then(|l| ss.find_syntax_by_first_line(l))
        })
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    let theme = &ts.themes[if light {
        "InspiredGitHub"
    } else {
        "base16-ocean.dark"
    }];
    let mut h = HighlightLines::new(syntax, theme);
    let mut byte = 0;
    let mut tokens = vec![];
    for line in LinesWithEndings::from(text) {
        if let Ok(ranges) = h.highlight_line(line, ss) {
            for (style, part) in ranges {
                let end = byte + part.len();
                tokens.push(Token {
                    start: byte,
                    end,
                    fg: format!(
                        "#{:02x}{:02x}{:02x}",
                        style.foreground.r, style.foreground.g, style.foreground.b
                    ),
                    bold: style.font_style.contains(FontStyle::BOLD),
                    italic: style.font_style.contains(FontStyle::ITALIC),
                });
                byte = end;
            }
        } else {
            byte += line.len();
        }
    }
    tokens
}

use crate::{layout::View, services::HighlightJob, App};

impl App {
    pub(super) fn schedule_highlight(&mut self) {
        if self.highlight_pending.len() >= 2 {
            return;
        }
        let light = self.light_theme();
        let mut ids = self
            .layout
            .panes()
            .iter()
            .filter_map(|p| match self.layout.view(*p) {
                Some(View::Editor(id)) => Some(self.views[id].document),
                _ => None,
            })
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids.dedup();
        for id in ids {
            let d = &self.documents[&id];
            if self.highlight_pending.contains_key(&id)
                || self
                    .tokens
                    .get(&id)
                    .is_some_and(|(g, l, _)| *g == d.generation && *l == light)
            {
                continue;
            }
            let job = HighlightJob {
                id,
                generation: d.generation,
                text: d.text.clone(),
                path: d.path.clone(),
                light,
            };
            if self.services.highlight.try_send(job).is_ok() {
                self.highlight_pending.insert(id, (d.generation, light));
            } else {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn extended_grammars_cover_common_project_files() {
        for (name, text) in [
            ("Cargo.toml", "[package]\nname = \"x\"\n"),
            ("app.ts", "const x: number = 1;\n"),
            ("Dockerfile", "FROM alpine\n"),
            ("main.rs", "fn main() {}\n"),
        ] {
            let tokens = super::highlight(text, Some(std::path::Path::new(name)), false);
            let colours: std::collections::BTreeSet<_> =
                tokens.iter().map(|t| t.fg.clone()).collect();
            assert!(colours.len() > 1, "{name} was not highlighted");
        }
    }
}
