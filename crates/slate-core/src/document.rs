use crate::{
    fsio::{self, save_error, Baseline, SaveErrorKind, WriteOptions},
    text_format::{self, TextFormat},
};
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
    /// Editing is disabled: inspection views (with a label) and `--view` files.
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
    /// How the text is stored on disk, and how it was last saved.
    #[serde(default)]
    pub format: TextFormat,
    #[serde(default)]
    saved_format: TextFormat,
    /// The disk content this buffer was read from or last written to. `None`
    /// for untitled buffers and files that do not exist yet.
    #[serde(default)]
    pub(crate) disk: Option<Baseline>,
    /// A recovery entry that stores only a reference to a clean file.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) reference: bool,
    /// The file exists but the current user cannot write it.
    #[serde(skip)]
    pub file_read_only: bool,
    #[serde(skip)]
    pub notice: Option<String>,
    #[serde(skip)]
    pub generation: u64,
    #[serde(skip)]
    undo: std::collections::VecDeque<Revision>,
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
            && self.undo.back().is_some_and(|r| {
                r.removed.is_empty() && r.start + r.inserted.len() == start && r.after == cursor
            });
        self.replace(start, end, value, cursor)?;
        if merge && self.undo.len() >= 2 {
            let tail = self.undo.pop_back().unwrap();
            let prior = self.undo.back_mut().unwrap();
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
    /// The line containing a byte offset.
    pub fn line_of(&self, offset: usize) -> usize {
        self.ensure_lines();
        let lines = self.lines.borrow();
        lines
            .as_ref()
            .unwrap()
            .1
            .partition_point(|start| *start <= offset)
            .saturating_sub(1)
    }
    /// Byte range of a line, excluding its line break.
    pub fn line_range(&self, row: usize) -> (usize, usize) {
        let start = self.line_offset(row);
        let end = if row + 1 < self.line_count() {
            self.line_offset(row + 1) - 1
        } else {
            self.text.len()
        };
        (start, end.max(start))
    }
    pub fn line_col(&self, cursor: usize, tab: usize) -> (usize, usize) {
        let row = self.line_of(cursor);
        let start = self.line_offset(row);
        (row, display_width_with_tabs(&self.text[start..cursor], tab))
    }
    pub fn at_line_col(&self, row: usize, col: usize, tab: usize) -> usize {
        if row >= self.line_count() {
            return self.text.len();
        }
        let (start, end) = self.line_range(row);
        start + column_offset(&self.text[start..end], col, tab)
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
            format: TextFormat::default(),
            saved_format: TextFormat::default(),
            disk: None,
            reference: false,
            file_read_only: false,
            notice: None,
            generation: 0,
            undo: Default::default(),
            redo: vec![],
        }
    }
    /// A named buffer for a path that does not exist yet; saving creates it.
    pub fn new_file(path: &Path) -> Result<Self> {
        let path = absolute(path)?;
        if path.is_dir() {
            bail!("{} is a directory", path.display());
        }
        let mut document = Self::scratch();
        document.notice = Some(format!("New file {}", path.display()));
        document.path = Some(path);
        Ok(document)
    }
    /// Text that did not come from a file, such as standard input.
    pub fn from_bytes(bytes: &[u8], label: &str) -> Result<Self> {
        if bytes.len() > MAX_FILE_BYTES {
            bail!("Prototype file limit is 16 MiB");
        }
        let decoded = text_format::decode(bytes, None)?;
        let mut document = Self::scratch();
        document.text = decoded.text;
        document.format = decoded.format.clone();
        document.saved_format = decoded.format;
        document.notice = Some(
            decoded
                .notice
                .unwrap_or_else(|| format!("Read {} bytes from {label}", bytes.len())),
        );
        // Unsaved input: closing the buffer asks before discarding it.
        Ok(document)
    }
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with_encoding(path, None)
    }
    pub fn open_with_encoding(path: &Path, encoding: Option<&str>) -> Result<Self> {
        let path = path.canonicalize().context("Cannot resolve file")?;
        let meta = fs::metadata(&path)?;
        if meta.is_dir() {
            bail!("{} is a directory", path.display());
        }
        if meta.len() > MAX_FILE_BYTES as u64 {
            bail!("Prototype file limit is 16 MiB");
        }
        let bytes = fs::read(&path)?;
        let decoded = text_format::decode(&bytes, encoding)?;
        let mut document = Self::scratch();
        document.file_read_only = !fsio::writable(&path);
        document.saved = decoded.text.clone();
        document.text = decoded.text;
        document.format = decoded.format.clone();
        document.saved_format = decoded.format;
        document.disk = Some(Baseline::of(&bytes));
        document.notice = decoded.notice;
        document.path = Some(path);
        Ok(document)
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
        let dirty = self.unavailable || self.text != self.saved || self.format != self.saved_format;
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
        format!(
            "{}{}{}",
            name,
            if self.read_only || self.file_read_only {
                " [RO]"
            } else {
                ""
            },
            if self.dirty() { " *" } else { "" }
        )
    }
    fn locked(&self) -> Result<()> {
        if self.read_only {
            if self.label.is_some() {
                bail!("This inspection view is read-only");
            }
            bail!("Document is open read-only; toggle editing with toggle-read-only");
        }
        Ok(())
    }
    pub fn replace(&mut self, start: usize, end: usize, value: &str, cursor: usize) -> Result<()> {
        self.locked()?;
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
        self.undo.push_back(Revision {
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
            .back()
            .map(|r| r.removed.len() + r.inserted.len())
            .unwrap_or(0);
        while self.undo.len() > 1
            && (self.undo.len() > 10_000 || self.history_size > HISTORY_BUDGET)
        {
            let r = self.undo.pop_front().unwrap();
            self.history_size -= r.removed.len() + r.inserted.len();
        }
        self.redo.clear();
        self.replace_text(start, end, value);
        Ok(())
    }
    pub fn undo_change(&self) -> Option<(usize, usize, usize)> {
        self.undo
            .back()
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
        let mut r = self.undo.pop_back()?;
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
        self.undo.push_back(r);
        Some(position)
    }
    /// A detached copy of the content and its save state, without history.
    pub fn checkpoint(&self) -> Self {
        let mut copy = Self::scratch();
        copy.path = self.path.clone();
        copy.read_only = self.read_only;
        copy.label = self.label.clone();
        copy.text = self.text.clone();
        copy.saved = self.saved.clone();
        copy.unavailable = self.unavailable;
        copy.format = self.format.clone();
        copy.saved_format = self.saved_format.clone();
        copy.disk = self.disk.clone();
        copy.generation = self.generation;
        copy
    }
    /// The recovery representation: unsaved buffers keep their text and save
    /// baseline; clean files are stored only as a path to reopen.
    pub fn recovery_copy(&self) -> Self {
        if self.dirty() || self.path.is_none() || self.label.is_some() {
            return self.checkpoint();
        }
        let mut copy = Self::scratch();
        copy.path = self.path.clone();
        copy.read_only = self.read_only;
        copy.format = self.format.clone();
        copy.saved_format = self.format.clone();
        copy.reference = true;
        copy
    }
    pub fn accept_save(&mut self, saved: Self) {
        self.unavailable = false;
        self.typing_at = None;
        self.path = saved.path;
        self.saved = saved.saved;
        self.saved_format = saved.saved_format;
        self.disk = saved.disk;
        self.file_read_only = saved.file_read_only;
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
        self.format = self.saved_format.clone();
        self.generation += 1;
        self.undo.clear();
        self.redo.clear();
        self.history_size = 0;
        self.typing_at = None;
    }
    pub fn history_bytes(&self) -> usize {
        self.history_size
    }
    /// Change how the document is stored. The change is saved like an edit.
    pub fn set_format(&mut self, format: TextFormat) -> Result<()> {
        self.locked()?;
        text_format::encode(&self.text, &format)?;
        self.format = format;
        self.generation += 1;
        Ok(())
    }
    /// The bytes a save would write.
    pub fn encoded(&self) -> Result<Vec<u8>> {
        text_format::encode(&self.text, &self.format)
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
        self.save_with_options(destination, overwrite, WriteOptions::default())
    }
    pub(crate) fn save_with_options(
        &mut self,
        destination: Option<&Path>,
        overwrite: bool,
        options: WriteOptions,
    ) -> Result<()> {
        if self.label.is_some() {
            bail!("Inspection views cannot be saved");
        }
        self.typing_at = None;
        let path = destination
            .map(Path::to_path_buf)
            .or_else(|| self.path.clone())
            .context("Use Save As for an untitled document")?;
        let path = if path.exists() {
            path.canonicalize()?
        } else {
            absolute(&path)?
        };
        let same = self.path.as_ref() == Some(&path);
        let baseline = if same {
            self.disk.clone()
        } else if path.exists() {
            if !overwrite {
                return Err(save_error(
                    SaveErrorKind::Other,
                    "Save As refuses to overwrite an existing file",
                ));
            }
            Some(Baseline::of(&fs::read(&path)?))
        } else {
            None
        };
        let bytes = self.encoded()?;
        let written = fsio::write_file(&path, &bytes, baseline.as_ref(), options)?;
        self.disk = Some(written);
        self.path = Some(path);
        self.saved = self.text.clone();
        self.saved_format = self.format.clone();
        self.file_read_only = self.path.as_deref().is_some_and(|p| !fsio::writable(p));
        self.dirty_cache.set(None);
        self.unavailable = false;
        Ok(())
    }
    /// Record a save performed by an elevated helper.
    pub(crate) fn mark_saved_bytes(&mut self, bytes: &[u8]) {
        self.disk = Some(Baseline::of(bytes));
        self.saved = self.text.clone();
        self.saved_format = self.format.clone();
        self.dirty_cache.set(None);
        self.unavailable = false;
    }
    pub fn baseline_matches(&self, bytes: &[u8]) -> bool {
        self.disk.as_ref() == Some(&Baseline::of(bytes))
    }
}

/// An absolute, lexically clean path whose parent is canonical when it exists.
pub fn absolute(path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let name = path.file_name().context("Missing filename")?.to_owned();
    let parent = path.parent().unwrap_or(Path::new("/"));
    Ok(parent
        .canonicalize()
        .unwrap_or_else(|_| parent.to_path_buf())
        .join(name))
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
/// End of the line containing `cursor`, before any `\r\n` or `\n` break.
pub fn line_end(text: &str, cursor: usize) -> usize {
    let end = text[cursor..]
        .find('\n')
        .map(|i| i + cursor)
        .unwrap_or(text.len());
    if end > cursor && text.as_bytes()[end - 1] == b'\r' && end < text.len() {
        end - 1
    } else {
        end
    }
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
/// Display columns of a line prefix. Plain ASCII avoids grapheme segmentation,
/// so very long lines stay cheap.
pub fn display_width_with_tabs(text: &str, tab: usize) -> usize {
    let tab = tab.max(1);
    if text.is_ascii() {
        let mut col = 0;
        for chunk in text.split('\t').enumerate() {
            if chunk.0 > 0 {
                col += tab - col % tab;
            }
            // Control characters render as one replacement cell; `\r` is hidden.
            col += chunk.1.len() - chunk.1.bytes().filter(|b| *b == b'\r').count();
        }
        return col;
    }
    let mut col = 0;
    for g in text.graphemes(true) {
        col += grapheme_width(g, col, tab);
    }
    col
}
pub(crate) fn grapheme_width(g: &str, col: usize, tab: usize) -> usize {
    if g == "\t" {
        tab - col % tab
    } else if g == "\r" || g == "\r\n" {
        0
    } else {
        // Matches the renderer: every visible grapheme occupies at least one cell.
        UnicodeWidthStr::width(g).max(1)
    }
}
/// Byte offset within a single line closest to a display column.
pub fn column_offset(line: &str, col: usize, tab: usize) -> usize {
    let tab = tab.max(1);
    if line.is_ascii() && !line.contains(['\t', '\r']) {
        return col.min(line.len());
    }
    let mut x = 0;
    for (i, g) in line.grapheme_indices(true) {
        if g == "\r" || g == "\r\n" {
            return i;
        }
        let w = grapheme_width(g, x, tab);
        if x + w > col {
            return i;
        }
        x += w;
    }
    line.len()
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
    start + column_offset(&text[start..end], col, tab)
}
