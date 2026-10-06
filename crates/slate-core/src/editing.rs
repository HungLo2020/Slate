use crate::{
    document::{line_end, line_start},
    App,
};
use anyhow::{bail, Context, Result};

impl App {
    pub(super) fn find(&mut self, backward: bool) -> Result<()> {
        let id = self.active_editor().context("Focus an editor first")?;
        let v = &self.views[&id];
        let text = &self.documents[&v.document].text;
        let matches = self
            .search
            .regex()?
            .find_iter(text)
            .map(|m| (m.start(), m.end()))
            .collect::<Vec<_>>();
        let cursor = if backward {
            v.anchor.map(|a| a.min(v.cursor)).unwrap_or(v.cursor)
        } else {
            v.cursor
        };
        let found = if backward {
            matches
                .iter()
                .rev()
                .find(|(_, end)| *end <= cursor)
                .or_else(|| matches.last())
        } else {
            matches
                .iter()
                .find(|(start, _)| *start >= cursor)
                .or_else(|| matches.first())
        };
        let &(start, end) = found.context("No matches")?;
        let number = matches.iter().position(|m| *m == (start, end)).unwrap() + 1;
        let v = self.views.get_mut(&id).unwrap();
        v.anchor = Some(start);
        v.cursor = end;
        v.manual_scroll = false;
        self.status = format!("Match {number} of {}", matches.len());
        Ok(())
    }
    pub(super) fn replace_matches(&mut self, replacement: &str, all: bool) -> Result<()> {
        let id = self.active_editor().context("Focus an editor first")?;
        let v = self.views[&id].clone();
        let text = &self.documents[&v.document].text;
        let regex = self.search.regex()?;
        if all {
            let count = regex.find_iter(text).count();
            if count == 0 {
                bail!("No matches");
            }
            let value = regex
                .replace_all(text, regex::NoExpand(replacement))
                .into_owned();
            self.edit_range(id, 0, text.len(), &value, v.cursor)?;
            self.status = format!("Replaced {count} matches");
        } else {
            let selected = v.anchor.map(|a| (a.min(v.cursor), a.max(v.cursor)));
            let matching = selected.is_some_and(|(a, b)| {
                regex
                    .find_at(text, a)
                    .is_some_and(|m| m.start() == a && m.end() == b)
            });
            if !matching {
                self.find(false)?;
            }
            self.edit(id, replacement)?;
            self.status = "Replaced match; Enter replaces next · Ctrl-Enter replaces all".into();
        }
        Ok(())
    }
    pub(super) fn indent(&mut self, outdent: bool) -> Result<()> {
        let id = self.active_editor().context("Focus an editor first")?;
        let v = self.views[&id].clone();
        let text = &self.documents[&v.document].text;
        let a = v.anchor.unwrap_or(v.cursor).min(v.cursor);
        let b = v.anchor.unwrap_or(v.cursor).max(v.cursor);
        let start = line_start(text, a);
        let last = if b > a && b == line_start(text, b) {
            b - 1
        } else {
            b
        };
        let end = line_end(text, last);
        let unit = if self.preferences.insert_spaces {
            " ".repeat(self.preferences.indent_width)
        } else {
            "\t".into()
        };
        let value = text[start..end]
            .split('\n')
            .map(|line| {
                if outdent {
                    let remove = if line.starts_with('\t') {
                        1
                    } else {
                        line.bytes()
                            .take_while(|b| *b == b' ')
                            .take(self.preferences.indent_width)
                            .count()
                    };
                    line[remove..].to_string()
                } else {
                    format!("{unit}{line}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        self.edit_range(id, start, end, &value, v.cursor)?;
        let new_end = start + value.len();
        let view = self.views.get_mut(&id).unwrap();
        if v.anchor.is_some() {
            view.anchor = Some(start);
            view.cursor = new_end;
        } else {
            let delta = value.len() as isize - (end - start) as isize;
            view.cursor = v
                .cursor
                .saturating_add_signed(delta)
                .max(start)
                .min(new_end);
            view.anchor = None;
        }
        Ok(())
    }
}
