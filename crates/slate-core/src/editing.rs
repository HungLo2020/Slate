use crate::App;
use anyhow::{bail, Context, Result};
use std::sync::Arc;

impl App {
    /// Matches of the current search in a document, cached until the
    /// document or the query changes; finding next/previous is a lookup.
    pub(crate) fn search_matches(&mut self, doc: u64) -> Result<Arc<Vec<(usize, usize)>>> {
        let generation = self.documents[&doc].generation;
        let key = self.search.key();
        if let Some((cached_doc, cached_generation, cached_key, matches)) = &self.match_cache {
            if *cached_doc == doc && *cached_generation == generation && *cached_key == key {
                return Ok(matches.clone());
            }
        }
        let regex = self.search.rope_regex()?;
        let matches = Arc::new(crate::search::find_all(&regex, self.documents[&doc].rope()));
        self.match_cache = Some((doc, generation, key, matches.clone()));
        Ok(matches)
    }
    pub(super) fn find(&mut self, backward: bool) -> Result<()> {
        let id = self.active_editor().context("Focus an editor first")?;
        let v = self.views[&id].clone();
        let matches = self.search_matches(v.document)?;
        let cursor = if backward {
            v.anchor.map(|a| a.min(v.cursor)).unwrap_or(v.cursor)
        } else {
            v.cursor
        };
        let index = if backward {
            let before = matches.partition_point(|(_, end)| *end <= cursor);
            before.checked_sub(1).or(matches.len().checked_sub(1))
        } else {
            let after = matches.partition_point(|(start, _)| *start < cursor);
            (after < matches.len())
                .then_some(after)
                .or((!matches.is_empty()).then_some(0))
        };
        let index = index.context("No matches")?;
        let (start, end) = matches[index];
        let view = self.views.get_mut(&id).unwrap();
        view.anchor = Some(start);
        view.cursor = end;
        view.manual_scroll = false;
        self.status = format!("Match {} of {}", index + 1, matches.len());
        Ok(())
    }
    pub(super) fn replace_matches(&mut self, replacement: &str, all: bool) -> Result<()> {
        let id = self.active_editor().context("Focus an editor first")?;
        let v = self.views[&id].clone();
        let matches = self.search_matches(v.document)?;
        if all {
            if matches.is_empty() {
                bail!("No matches");
            }
            // One undoable edit spanning the matches; replacement text is literal.
            let d = &self.documents[&v.document];
            let (first, last) = (matches[0].0, matches[matches.len() - 1].1);
            let mut value = String::with_capacity(last - first);
            let mut position = first;
            for (start, end) in matches.iter() {
                value.push_str(&d.slice(position, *start));
                value.push_str(replacement);
                position = *end;
            }
            self.edit_range(id, first, last, &value, v.cursor)?;
            self.status = format!(
                "Replaced {}",
                crate::counted(matches.len(), "match", "matches")
            );
        } else {
            let selected = v.anchor.map(|a| (a.min(v.cursor), a.max(v.cursor)));
            let matching = selected.is_some_and(|range| matches.binary_search(&range).is_ok());
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
        let value = d
            .slice(start, end)
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
