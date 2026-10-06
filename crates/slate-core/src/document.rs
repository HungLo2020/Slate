use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub const MAX_FILE_BYTES: usize = 16 * 1024 * 1024;
const HISTORY_BUDGET: usize = 32 * 1024 * 1024;

#[derive(Clone)]
struct Revision {
    start: usize,
    removed: String,
    inserted: String,
    before: usize,
    after: usize,
}

#[derive(Serialize, Deserialize)]
pub struct Document {
    pub path: Option<PathBuf>,
    pub text: String,
    saved: String,
    #[serde(default)]
    unavailable: bool,
    #[serde(skip)]
    pub generation: u64,
    #[serde(skip)]
    undo: Vec<Revision>,
    #[serde(skip)]
    redo: Vec<Revision>,
}

impl Document {
    pub fn scratch() -> Self {
        Self {
            path: None,
            text: String::new(),
            saved: String::new(),
            unavailable: false,
            generation: 0,
            undo: vec![],
            redo: vec![],
        }
    }
    pub fn open(path: &Path) -> Result<Self> {
        let path = path.canonicalize().context("Cannot resolve file")?;
        if fs::metadata(&path)?.len() > MAX_FILE_BYTES as u64 {
            bail!("Prototype file limit is 16 MiB");
        }
        let bytes = fs::read(&path)?;
        let text = String::from_utf8(bytes).context("Slate currently edits UTF-8 text only")?;
        Ok(Self {
            path: Some(path),
            saved: text.clone(),
            unavailable: false,
            text,
            generation: 0,
            undo: vec![],
            redo: vec![],
        })
    }
    pub fn dirty(&self) -> bool {
        self.unavailable || self.text != self.saved
    }
    pub fn title(&self) -> String {
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into());
        format!("{}{}", name, if self.dirty() { " *" } else { "" })
    }
    pub fn replace(&mut self, start: usize, end: usize, value: &str, cursor: usize) -> Result<()> {
        if start > end
            || end > self.text.len()
            || !self.text.is_char_boundary(start)
            || !self.text.is_char_boundary(end)
        {
            bail!("Invalid text range");
        }
        if self.text.len() - (end - start) + value.len() > MAX_FILE_BYTES {
            bail!("Prototype buffer limit is 16 MiB");
        }
        if &self.text[start..end] == value {
            return Ok(());
        }
        self.undo.push(Revision {
            start,
            removed: self.text[start..end].into(),
            inserted: value.into(),
            before: cursor,
            after: start + value.len(),
        });
        let mut total: usize = self
            .undo
            .iter()
            .map(|r| r.removed.len() + r.inserted.len())
            .sum();
        while self.undo.len() > 1 && (self.undo.len() > 10_000 || total > HISTORY_BUDGET) {
            let r = self.undo.remove(0);
            total -= r.removed.len() + r.inserted.len();
        }
        self.redo.clear();
        self.text.replace_range(start..end, value);
        self.generation += 1;
        Ok(())
    }
    pub fn undo_change(&self) -> Option<(usize, usize, usize)> {
        self.undo
            .last()
            .map(|r| (r.start, r.inserted.len(), r.removed.len()))
    }
    pub fn redo_change(&self) -> Option<(usize, usize, usize)> {
        self.redo
            .last()
            .map(|r| (r.start, r.removed.len(), r.inserted.len()))
    }
    pub fn undo(&mut self, cursor: usize) -> Option<usize> {
        let mut r = self.undo.pop()?;
        self.text
            .replace_range(r.start..r.start + r.inserted.len(), &r.removed);
        r.after = cursor;
        let position = r.before.min(self.text.len());
        self.redo.push(r);
        self.generation += 1;
        Some(position)
    }
    pub fn redo(&mut self, cursor: usize) -> Option<usize> {
        let mut r = self.redo.pop()?;
        self.text
            .replace_range(r.start..r.start + r.removed.len(), &r.inserted);
        r.before = cursor;
        let position = r.after.min(self.text.len());
        self.undo.push(r);
        self.generation += 1;
        Some(position)
    }
    pub fn checkpoint(&self) -> Self {
        Self {
            path: self.path.clone(),
            text: self.text.clone(),
            saved: self.saved.clone(),
            unavailable: self.unavailable,
            generation: self.generation,
            undo: vec![],
            redo: vec![],
        }
    }
    pub fn accept_save(&mut self, saved: Self) {
        self.unavailable = false;
        self.path = saved.path;
        self.saved = saved.saved;
        self.generation += 1;
    }
    pub fn mark_unavailable(&mut self) {
        self.unavailable = true;
    }
    pub fn valid_recovery(&self) -> bool {
        self.text.len() <= MAX_FILE_BYTES && self.saved.len() <= MAX_FILE_BYTES
    }
    pub fn discard_changes(&mut self) {
        self.unavailable = false;
        self.text = self.saved.clone();
    }
    pub fn history_bytes(&self) -> usize {
        self.undo
            .iter()
            .chain(&self.redo)
            .map(|r| r.removed.len() + r.inserted.len())
            .sum()
    }
    pub fn save(&mut self, destination: Option<&Path>) -> Result<()> {
        let mut path = destination
            .map(Path::to_path_buf)
            .or_else(|| self.path.clone())
            .context("Use Save As for an untitled document")?;
        if path.exists() {
            path = path.canonicalize()?;
        } else {
            path = path
                .parent()
                .unwrap_or(Path::new("."))
                .canonicalize()?
                .join(path.file_name().context("Missing filename")?);
        }
        let same = self.path.as_ref() == Some(&path);
        if path.exists() {
            if !same {
                bail!("Save As refuses to overwrite an existing file");
            }
            if self.saved.as_bytes() != fs::read(&path)?.as_slice() {
                bail!("File changed on disk. Save As to a new path or reopen after preserving your changes");
            }
        } else if same {
            bail!("File was removed externally. Save As to a new path");
        }
        let parent = path.parent().context("Missing parent directory")?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        if let Ok(meta) = fs::metadata(&path) {
            temp.as_file().set_permissions(meta.permissions())?;
        }
        use std::io::Write;
        temp.write_all(self.text.as_bytes())?;
        temp.as_file().sync_all()?;
        // Recheck immediately before atomic replacement; ordinary edits are never silently overwritten.
        if same
            && self.saved.as_bytes()
                != fs::read(&path)
                    .context("File removed during save")?
                    .as_slice()
        {
            bail!("File changed during save");
        }
        if same {
            temp.persist(&path).map_err(|e| e.error)?;
        } else {
            temp.persist_noclobber(&path).map_err(|e| e.error)?;
        }
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        self.path = Some(path);
        self.saved = self.text.clone();
        self.unavailable = false;

        Ok(())
    }
}

pub fn previous(text: &str, cursor: usize) -> usize {
    text[..cursor]
        .grapheme_indices(true)
        .next_back()
        .map(|(i, _)| i)
        .unwrap_or(0)
}
pub fn next(text: &str, cursor: usize) -> usize {
    cursor
        + text[cursor..]
            .graphemes(true)
            .next()
            .map(str::len)
            .unwrap_or(0)
}
pub fn line_start(text: &str, cursor: usize) -> usize {
    text[..cursor].rfind('\n').map(|i| i + 1).unwrap_or(0)
}
pub fn line_end(text: &str, cursor: usize) -> usize {
    text[cursor..]
        .find('\n')
        .map(|i| i + cursor)
        .unwrap_or(text.len())
}
pub fn line_col(text: &str, cursor: usize) -> (usize, usize) {
    line_col_with_tabs(text, cursor, 4)
}
pub fn line_col_with_tabs(text: &str, cursor: usize, tab: usize) -> (usize, usize) {
    let start = line_start(text, cursor);
    (
        text[..start].bytes().filter(|b| *b == b'\n').count(),
        display_width_with_tabs(&text[start..cursor], tab),
    )
}
pub fn display_width(text: &str) -> usize {
    display_width_with_tabs(text, 4)
}
pub fn display_width_with_tabs(text: &str, tab: usize) -> usize {
    let mut col = 0;
    for g in text.graphemes(true) {
        col += if g == "\t" {
            tab - col % tab
        } else {
            UnicodeWidthStr::width(g)
        };
    }
    col
}
pub fn at_line_col(text: &str, row: usize, col: usize) -> usize {
    at_line_col_with_tabs(text, row, col, 4)
}
pub fn at_line_col_with_tabs(text: &str, row: usize, col: usize, tab: usize) -> usize {
    let start = text
        .match_indices('\n')
        .nth(row.saturating_sub(1))
        .map(|(i, _)| i + 1)
        .filter(|_| row > 0)
        .unwrap_or(if row == 0 { 0 } else { text.len() });
    let end = line_end(text, start);
    let mut x = 0;
    for (i, g) in text[start..end].grapheme_indices(true) {
        let w = if g == "\t" {
            tab - x % tab
        } else {
            UnicodeWidthStr::width(g)
        };
        if x + w > col {
            return start + i;
        }
        x += w;
    }
    end
}
