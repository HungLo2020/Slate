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
    pub(crate) text: String,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(skip)]
    lines: std::cell::RefCell<Option<(u64, Vec<usize>)>>,
    #[serde(skip)]
    dirty_cache: std::cell::Cell<Option<(u64, bool)>>,
    #[serde(skip)]
    typing_at: Option<std::time::Instant>,
    #[serde(skip)]
    history_size: usize,
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
    pub fn from_text(text: String) -> Result<Self> {
        if text.len() > MAX_FILE_BYTES {
            bail!("Prototype buffer limit is 16 MiB");
        }
        let mut document = Self::scratch();
        document.text = text;
        Ok(document)
    }
    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn inspection(label: String, text: String) -> Self {
        let mut doc = Self::scratch();
        doc.text = text;
        doc.saved = doc.text.clone();
        doc.label = Some(label);
        doc.read_only = true;
        doc
    }
    pub fn set_text(&mut self, text: String) -> Result<()> {
        self.replace(0, self.text.len(), &text, 0)
    }
    pub fn break_typing(&mut self) {
        self.typing_at = None;
    }
    pub fn replace_typing(
        &mut self,
        start: usize,
        end: usize,
        value: &str,
        cursor: usize,
    ) -> Result<()> {
        let merge = start == end
            && !value.contains(['\n', '\r'])
            && self
                .typing_at
                .is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(750))
            && self.undo.last().is_some_and(|r| {
                r.removed.is_empty() && r.start + r.inserted.len() == start && r.after == cursor
            });
        self.replace(start, end, value, cursor)?;
        if merge && self.undo.len() >= 2 {
            let tail = self.undo.pop().unwrap();
            let prior = self.undo.last_mut().unwrap();
            prior.inserted.push_str(&tail.inserted);
            prior.after = tail.after;
        }
        self.typing_at = Some(std::time::Instant::now());
        Ok(())
    }
    fn ensure_lines(&self) {
        let mut lines = self.lines.borrow_mut();
        if lines.as_ref().is_none_or(|(g, _)| *g != self.generation) {
            let mut starts = vec![0];
            starts.extend(
                self.text
                    .bytes()
                    .enumerate()
                    .filter_map(|(i, b)| (b == b'\n').then_some(i + 1)),
            );
            *lines = Some((self.generation, starts));
        }
    }
    pub fn line_count(&self) -> usize {
        self.ensure_lines();
        self.lines.borrow().as_ref().unwrap().1.len()
    }
    pub fn line_offset(&self, row: usize) -> usize {
        self.ensure_lines();
        self.lines
            .borrow()
            .as_ref()
            .unwrap()
            .1
            .get(row)
            .copied()
            .unwrap_or(self.text.len())
    }
    pub fn line_col(&self, cursor: usize, tab: usize) -> (usize, usize) {
        self.ensure_lines();
        let lines = self.lines.borrow();
        let starts = &lines.as_ref().unwrap().1;
        let row = starts
            .partition_point(|start| *start <= cursor)
            .saturating_sub(1);
        (
            row,
            display_width_with_tabs(&self.text[starts[row]..cursor], tab),
        )
    }
    pub fn at_line_col(&self, row: usize, col: usize, tab: usize) -> usize {
        let start = self.line_offset(row);
        start + at_line_col_with_tabs(&self.text[start..], 0, col, tab)
    }
    fn replace_text(&mut self, start: usize, end: usize, value: &str) {
        self.ensure_lines();
        let (_, mut starts) = self.lines.get_mut().take().unwrap();
        let first = starts.partition_point(|p| *p <= start);
        let last = starts.partition_point(|p| *p <= end);
        let delta = value.len() as isize - (end - start) as isize;
        for offset in &mut starts[last..] {
            *offset = offset.saturating_add_signed(delta);
        }
        starts.splice(
            first..last,
            value
                .bytes()
                .enumerate()
                .filter_map(|(i, b)| (b == b'\n').then_some(start + i + 1)),
        );
        self.text.replace_range(start..end, value);
        self.generation += 1;
        *self.lines.get_mut() = Some((self.generation, starts));
    }

    pub fn scratch() -> Self {
        Self {
            path: None,
            read_only: false,
            label: None,
            lines: Default::default(),
            dirty_cache: Default::default(),
            typing_at: None,
            history_size: 0,
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
            read_only: false,
            label: None,
            lines: Default::default(),
            dirty_cache: Default::default(),
            typing_at: None,
            history_size: 0,
            saved: text.clone(),
            unavailable: false,
            text,
            generation: 0,
            undo: vec![],
            redo: vec![],
        })
    }
    pub fn dirty(&self) -> bool {
        if self.read_only {
            return false;
        }
        if let Some((generation, dirty)) = self.dirty_cache.get() {
            if generation == self.generation {
                return dirty;
            }
        }
        let dirty = self.unavailable || self.text != self.saved;
        self.dirty_cache.set(Some((self.generation, dirty)));
        dirty
    }
    pub fn title(&self) -> String {
        if let Some(label) = &self.label {
            return label.clone();
        }
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".into());
        format!("{}{}", name, if self.dirty() { " *" } else { "" })
    }
    pub fn replace(&mut self, start: usize, end: usize, value: &str, cursor: usize) -> Result<()> {
        if self.read_only {
            bail!("This inspection view is read-only");
        }
        self.typing_at = None;
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
        self.history_size = self.history_size.saturating_sub(
            self.redo
                .iter()
                .map(|r| r.removed.len() + r.inserted.len())
                .sum::<usize>(),
        );
        self.history_size += self
            .undo
            .last()
            .map(|r| r.removed.len() + r.inserted.len())
            .unwrap_or(0);
        while self.undo.len() > 1
            && (self.undo.len() > 10_000 || self.history_size > HISTORY_BUDGET)
        {
            let r = self.undo.remove(0);
            self.history_size -= r.removed.len() + r.inserted.len();
        }
        self.redo.clear();
        self.replace_text(start, end, value);
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
        if self.read_only {
            return None;
        }
        self.typing_at = None;
        let mut r = self.undo.pop()?;
        self.replace_text(r.start, r.start + r.inserted.len(), &r.removed);
        r.after = cursor;
        let position = r.before.min(self.text.len());
        self.redo.push(r);
        Some(position)
    }
    pub fn redo(&mut self, cursor: usize) -> Option<usize> {
        if self.read_only {
            return None;
        }
        self.typing_at = None;
        let mut r = self.redo.pop()?;
        self.replace_text(r.start, r.start + r.removed.len(), &r.inserted);
        r.before = cursor;
        let position = r.after.min(self.text.len());
        self.undo.push(r);
        Some(position)
    }
    pub fn checkpoint(&self) -> Self {
        Self {
            path: self.path.clone(),
            read_only: self.read_only,
            label: self.label.clone(),
            lines: Default::default(),
            dirty_cache: Default::default(),
            typing_at: None,
            history_size: 0,
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
        self.typing_at = None;
        self.path = saved.path;
        self.saved = saved.saved;
        self.generation += 1;
    }
    pub fn mark_unavailable(&mut self) {
        self.unavailable = true;
        self.dirty_cache.set(None);
    }
    pub fn valid_recovery(&self) -> bool {
        self.text.len() <= MAX_FILE_BYTES && self.saved.len() <= MAX_FILE_BYTES
    }
    pub fn discard_changes(&mut self) {
        self.unavailable = false;
        self.text = self.saved.clone();
        self.generation += 1;
        self.undo.clear();
        self.redo.clear();
        self.history_size = 0;
        self.typing_at = None;
    }
    pub fn history_bytes(&self) -> usize {
        self.history_size
    }
    pub fn save(&mut self, destination: Option<&Path>) -> Result<()> {
        self.save_with_overwrite(destination, false)
    }
    // Only an explicitly confirmed Save As may replace a different existing
    // file. Normal saves still compare against the document's disk baseline.
    pub(crate) fn save_with_overwrite(
        &mut self,
        destination: Option<&Path>,
        overwrite: bool,
    ) -> Result<()> {
        if self.read_only {
            bail!("Inspection views cannot be saved");
        }
        self.typing_at = None;
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
        let disk_baseline = if path.exists() {
            if !same && !overwrite {
                bail!("Save As refuses to overwrite an existing file");
            }
            let contents = fs::read(&path)?;
            if same && self.saved.as_bytes() != contents.as_slice() {
                bail!("File changed on disk. Save As to a new path or reopen after preserving your changes");
            }
            Some(contents)
        } else if same {
            bail!("File was removed externally. Save As to a new path");
        } else {
            None
        };
        let parent = path.parent().context("Missing parent directory")?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        if let Ok(meta) = fs::metadata(&path) {
            temp.as_file().set_permissions(meta.permissions())?;
        }
        use std::io::Write;
        temp.write_all(self.text.as_bytes())?;
        temp.as_file().sync_all()?;
        // Recheck immediately before atomic replacement; ordinary edits are never silently overwritten.
        if let Some(baseline) = &disk_baseline {
            if baseline.as_slice()
                != fs::read(&path)
                    .context("File removed during save")?
                    .as_slice()
            {
                bail!("File changed during save");
            }
        }
        if disk_baseline.is_some() {
            temp.persist(&path).map_err(|e| e.error)?;
        } else {
            temp.persist_noclobber(&path).map_err(|e| e.error)?;
        }
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        self.path = Some(path);
        self.saved = self.text.clone();
        self.dirty_cache.set(None);
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
