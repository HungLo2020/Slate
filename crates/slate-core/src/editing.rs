use crate::App;
use anyhow::{Context, Result};

impl App {
    pub(super) fn find(&mut self, backward: bool) -> Result<()> {
        self.document_search(crate::document_search::Mode::Find { backward })
    }
    pub(super) fn replace_matches(&mut self, replacement: &str, all: bool) -> Result<()> {
        self.document_search(crate::document_search::Mode::Replace {
            text: replacement.into(),
            all,
        })
    }
    pub(super) fn indent(&mut self, outdent: bool) -> Result<()> {
        let id = self.active_editor().context("Focus an editor first")?;
        let v = self.views[&id].clone();
        let d = &self.documents[&v.document];
        let a = v.anchor.unwrap_or(v.cursor).min(v.cursor);
        let b = v.anchor.unwrap_or(v.cursor).max(v.cursor);
        let first = d.line_of(a);
        let mut last = d.line_of(b);
        // A selection ending at a line start does not include that line.
        if b > a && last > first && b == d.line_offset(last) {
            last -= 1;
        }
        let start = d.line_offset(first);
        let end = d.line_range(last).1;
        let unit = if self.preferences.insert_spaces {
            " ".repeat(self.preferences.indent_width)
        } else {
            "\t".into()
        };
        anyhow::ensure!(
            last - first < 10_000,
            "Indentation is limited to 10,000 lines per operation"
        );
        let mut edits = Vec::new();
        for line in first..=last {
            let at = d.line_offset(line);
            let end = d.line_range(line).1;
            if outdent {
                let slice = d.rope().byte_slice(at..end);
                let remove = if slice.chars().next() == Some('\t') {
                    1
                } else {
                    slice
                        .chars()
                        .take(self.preferences.indent_width)
                        .take_while(|c| *c == ' ')
                        .count()
                };
                if remove > 0 {
                    edits.push((at, at + remove, String::new()));
                }
            } else {
                edits.push((at, at, unit.clone()));
            }
        }
        let delta = edits
            .iter()
            .map(|(a, b, text)| text.len() as isize - (*b - *a) as isize)
            .sum::<isize>();
        self.replace_in_document(v.document, &edits)?;
        if v.anchor.is_some() {
            let view = self.views.get_mut(&id).unwrap();
            view.anchor = Some(start);
            view.cursor = end.saturating_add_signed(delta);
        }
        Ok(())
    }
}
