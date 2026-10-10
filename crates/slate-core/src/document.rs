use crate::{
    fsio::{self, save_error, Baseline, SaveErrorKind, WriteOptions},
    text_format::{self, TextFormat},
};
use anyhow::{bail, Context, Result};
use ropey::{Rope, RopeSlice};
use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete, UnicodeSegmentation};
use unicode_width::UnicodeWidthStr;

/// Documents are ropes: edits cost O(log n) regardless of size. The limit
/// guards memory, not performance.
pub const MAX_FILE_BYTES: usize = 1024 * 1024 * 1024;
/// Undo history is trimmed, oldest first and whole undo steps at a time,
/// beyond this many bytes, steps or revisions. The step being recorded is
/// never trimmed: one larger than the budget becomes the only step kept.
/// Revisions are capped too because each costs memory beyond its text and a
/// single multi-caret step can hold thousands of them.
const HISTORY_BUDGET: usize = 64 * 1024 * 1024;
const HISTORY_STEPS: usize = 10_000;
const HISTORY_REVISIONS: usize = 200_000;
/// Changes kept for language servers between drains; beyond this they
/// receive the whole document instead.
const CHANGE_LIMIT: usize = 4096;
/// Inserted text recorded for language servers before falling back to a
/// full update.
const CHANGE_BYTES: usize = 8 * 1024 * 1024;
/// Recent splices kept for caches that follow edits (wrapped rows).
const SPLICES: usize = 256;

static VERSIONS: AtomicU64 = AtomicU64::new(1);
fn new_version() -> u64 {
    VERSIONS.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone)]
struct Revision {
    start: usize,
    removed: String,
    inserted: String,
    before: usize,
    after: usize,
    /// Content versions on either side of the edit, so undoing back to the
    /// saved state makes the document clean again.
    version_before: u64,
    version_after: u64,
    /// Revisions sharing a nonzero group undo and redo together.
    group: u64,
}

/// Whether a revision begins a new undo step after `previous`.
fn starts_step(previous: Option<&Revision>, r: &Revision) -> bool {
    r.group == 0 || previous.is_none_or(|p| p.group != r.group)
}

/// The leading revisions that undo or redo together.
fn group_changes<'a>(
    revisions: impl Iterator<Item = &'a Revision>,
    change: impl Fn(&Revision) -> (usize, usize, usize),
) -> Vec<(usize, usize, usize)> {
    let mut revisions = revisions.peekable();
    let mut changes = Vec::new();
    while let Some(r) = revisions.next() {
        changes.push(change(r));
        if r.group == 0 || revisions.peek().is_none_or(|n| n.group != r.group) {
            break;
        }
    }
    changes
}

/// One replacement as caches see it: bytes `start..old_end` of the text
/// before it became `new_len` bytes; line breaks removed and inserted.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Splice {
    pub start: usize,
    pub old_end: usize,
    pub new_len: usize,
    pub old_breaks: usize,
    pub new_breaks: usize,
}

/// One edit, with positions in the text before it (lines and UTF-16
/// columns, as the Language Server Protocol counts them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextChange {
    pub start: usize,
    pub old_end: usize,
    pub start_position: (usize, usize),
    pub old_end_position: (usize, usize),
    pub text: String,
}

mod rope_text {
    use ropey::Rope;
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(rope: &Rope, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(rope)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Rope, D::Error> {
        Ok(Rope::from(String::deserialize(deserializer)?.as_str()))
    }
}

#[derive(Serialize, Deserialize)]
pub struct Document {
    pub path: Option<PathBuf>,
    #[serde(with = "rope_text")]
    pub(crate) text: Rope,
    /// Editing is disabled: inspection views (with a label) and `--view` files.
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(skip)]
    typing_at: Option<std::time::Instant>,
    #[serde(skip)]
    history_size: usize,
    #[serde(with = "rope_text")]
    saved: Rope,
    #[serde(skip)]
    version: u64,
    #[serde(skip)]
    saved_version: u64,
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
    /// Placeholder whose payload must be hydrated before recovery validation.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) recovery_external: bool,
    /// The file exists but the current user cannot write it.
    #[serde(skip)]
    pub file_read_only: bool,
    /// Size, modification time and inode when the file was read or written;
    /// the change watcher only rereads a file when its stamp moves.
    #[serde(skip)]
    pub(crate) stamp: Option<FileStamp>,
    #[serde(skip)]
    pub notice: Option<String>,
    #[serde(skip)]
    pub generation: u64,
    #[serde(skip)]
    undo: std::collections::VecDeque<Revision>,
    /// Undo steps in `undo` (a group of revisions counts once).
    #[serde(skip)]
    undo_steps: usize,
    #[serde(skip)]
    redo: Vec<Revision>,
    /// Edits since language servers last synchronized; `None` after overflow.
    #[serde(skip)]
    changes: Option<Vec<TextChange>>,
    /// Line-level edits for the highlighter: (first line, old lines, new lines).
    #[serde(skip)]
    line_edits: Option<Vec<(usize, usize, usize)>>,
    /// Text held in `changes`, bounded like its length.
    #[serde(skip)]
    change_bytes: usize,
    /// Inspection documents (output, debugger) are never synchronized to a
    /// language server, so their edits are not recorded.
    #[serde(skip)]
    quiet: bool,
    /// Identifies the text: a new value whenever it changes in any way.
    #[serde(skip, default = "new_version")]
    serial: u64,
    /// The latest splices, each with the serial it was applied to.
    #[serde(skip)]
    splices: std::collections::VecDeque<(u64, Splice)>,
}

impl Document {
    pub fn from_text(text: String) -> Result<Self> {
        if text.len() > MAX_FILE_BYTES {
            bail!("Documents are limited to 1 GiB");
        }
        let mut document = Self::scratch();
        document.text = Rope::from(text.as_str());
        if !text.is_empty() {
            document.version = new_version();
        }
        Ok(document)
    }
    /// A copy of the whole text. Prefer `rope`, `slice` or `line_text`.
    pub fn text(&self) -> String {
        self.text.to_string()
    }
    pub fn rope(&self) -> &Rope {
        &self.text
    }
    pub fn len(&self) -> usize {
        self.text.len_bytes()
    }
    pub fn is_empty(&self) -> bool {
        self.text.len_bytes() == 0
    }
    /// Text in a byte range (borrowed when it lies within one rope chunk).
    pub fn slice(&self, start: usize, end: usize) -> Cow<'_, str> {
        let slice = self.text.byte_slice(start..end);
        match slice.as_str() {
            Some(s) => Cow::Borrowed(s),
            None => Cow::Owned(slice.to_string()),
        }
    }
    pub fn inspection(label: String, text: String) -> Self {
        let mut doc = Self::scratch();
        doc.text = Rope::from(text.as_str());
        doc.saved = doc.text.clone();
        doc.label = Some(label);
        doc.read_only = true;
        doc.quiet = true;
        doc.changes = None;
        doc
    }
    pub fn set_text(&mut self, text: String) -> Result<()> {
        self.replace(0, self.len(), &text, 0)
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
            let tail = self.pop_undo().unwrap();
            let prior = self.undo.back_mut().unwrap();
            prior.inserted.push_str(&tail.inserted);
            prior.after = tail.after;
            prior.version_after = tail.version_after;
        }
        self.typing_at = Some(std::time::Instant::now());
        Ok(())
    }

    // ----- Lines and positions -----

    pub fn line_count(&self) -> usize {
        self.text.len_lines()
    }
    /// Byte offset where a line starts (the end of the text past the last line).
    pub fn line_offset(&self, row: usize) -> usize {
        if row >= self.text.len_lines() {
            return self.len();
        }
        self.text.line_to_byte(row)
    }
    /// The line containing a byte offset.
    pub fn line_of(&self, offset: usize) -> usize {
        self.text.byte_to_line(offset.min(self.len()))
    }
    /// Byte range of a line, excluding its line break.
    pub fn line_range(&self, row: usize) -> (usize, usize) {
        let start = self.line_offset(row);
        let end = if row + 1 < self.line_count() {
            self.line_offset(row + 1) - 1
        } else {
            self.len()
        };
        (start, end.max(start))
    }
    /// A line's text without its line break.
    /// The leading spaces and tabs of a line.
    pub fn indent_of(&self, row: usize) -> String {
        self.line_text(row)
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect()
    }
    pub fn line_text(&self, row: usize) -> Cow<'_, str> {
        let (start, end) = self.line_range(row);
        self.slice(start, end)
    }
    pub fn line_col(&self, cursor: usize, tab: usize) -> (usize, usize) {
        let row = self.line_of(cursor);
        let start = self.line_offset(row);
        (
            row,
            slice_width(self.text.byte_slice(start..cursor.max(start)), tab),
        )
    }
    pub fn at_line_col(&self, row: usize, col: usize, tab: usize) -> usize {
        if row >= self.line_count() {
            return self.len();
        }
        let (start, end) = self.line_range(row);
        start + slice_column_offset(self.text.byte_slice(start..end), col, tab)
    }
    pub fn is_char_boundary(&self, offset: usize) -> bool {
        offset <= self.len()
            && (offset == self.len()
                || self
                    .text
                    .try_byte_to_char(offset)
                    .is_ok_and(|c| self.text.char_to_byte(c) == offset))
    }
    /// The closest character boundary at or before an offset.
    pub fn floor_boundary(&self, offset: usize) -> usize {
        let offset = offset.min(self.len());
        self.text.char_to_byte(self.text.byte_to_char(offset))
    }
    /// The closest character boundary at or after an offset.
    pub fn next_char_boundary_from(&self, offset: usize) -> usize {
        let floor = self.floor_boundary(offset);
        if floor >= offset || floor >= self.len() {
            floor
        } else {
            self.text.char_to_byte(self.text.byte_to_char(floor) + 1)
        }
    }
    pub fn previous_boundary(&self, offset: usize) -> usize {
        previous_grapheme(&self.text.slice(..), offset.min(self.len()))
    }
    pub fn next_boundary(&self, offset: usize) -> usize {
        next_grapheme(&self.text.slice(..), offset.min(self.len()))
    }
    /// (line, UTF-16 column) of a byte offset.
    pub fn position(&self, offset: usize) -> (usize, usize) {
        let offset = self.floor_boundary(offset);
        let line = self.text.byte_to_line(offset);
        let line_char = self.text.line_to_char(line);
        let char = self.text.byte_to_char(offset);
        (
            line,
            self.text.char_to_utf16_cu(char) - self.text.char_to_utf16_cu(line_char),
        )
    }
    /// The byte offset of (line, UTF-16 column), clamped to the line.
    pub fn offset_of(&self, line: usize, utf16: usize) -> usize {
        if line >= self.line_count() {
            return self.len();
        }
        let line_char = self.text.line_to_char(line);
        let (_, end) = self.line_range(line);
        let end_char = self.text.byte_to_char(end);
        let base = self.text.char_to_utf16_cu(line_char);
        let target = (base + utf16).min(self.text.char_to_utf16_cu(end_char));
        self.text.char_to_byte(self.text.utf16_cu_to_char(target))
    }
    /// UTF-16 code units before a byte offset (input methods count in UTF-16).
    pub fn utf16_offset(&self, offset: usize) -> usize {
        self.text
            .char_to_utf16_cu(self.text.byte_to_char(self.floor_boundary(offset)))
    }
    pub fn byte_of_utf16(&self, units: usize) -> Option<usize> {
        if units > self.text.len_utf16_cu() {
            return None;
        }
        Some(self.text.char_to_byte(self.text.utf16_cu_to_char(units)))
    }

    fn replace_text(&mut self, start: usize, end: usize, value: &str) {
        if self.quiet {
            self.changes = None;
        }
        if let Some(changes) = &mut self.changes {
            self.change_bytes += value.len();
            if changes.len() >= CHANGE_LIMIT || self.change_bytes > CHANGE_BYTES {
                // Too much to replay: the next sync sends the whole text.
                self.changes = None;
                self.change_bytes = 0;
            } else {
                let change = TextChange {
                    start,
                    old_end: end,
                    start_position: self.position(start),
                    old_end_position: self.position(end),
                    text: value.to_string(),
                };
                if let Some(changes) = &mut self.changes {
                    changes.push(change);
                }
            }
        }
        if let Some(edits) = &mut self.line_edits {
            if edits.len() >= CHANGE_LIMIT {
                self.line_edits = None;
            } else {
                let first_line = self.text.byte_to_line(start);
                let old = self.text.byte_to_line(end) - first_line + 1;
                let new = value.bytes().filter(|b| *b == b'\n').count() + 1;
                if let Some(edits) = &mut self.line_edits {
                    edits.push((first_line, old, new));
                }
            }
        }
        let splice = Splice {
            start,
            old_end: end,
            new_len: value.len(),
            old_breaks: self.text.byte_to_line(end) - self.text.byte_to_line(start),
            new_breaks: value.bytes().filter(|b| *b == b'\n').count(),
        };
        if self.splices.len() >= SPLICES {
            self.splices.pop_front();
        }
        self.splices.push_back((self.serial, splice));
        self.serial = new_version();
        let first = self.text.byte_to_char(start);
        let last = self.text.byte_to_char(end);
        self.text.remove(first..last);
        self.text.insert(first, value);
        self.generation += 1;
    }
    /// The identity of the current text (see `splices_since`).
    pub(crate) fn serial(&self) -> u64 {
        self.serial
    }
    /// The splices that turned the text with `serial` into the current one,
    /// in order; `None` when they are no longer known.
    pub(crate) fn splices_since(&self, serial: u64) -> Option<impl Iterator<Item = &Splice>> {
        let first = if serial == self.serial {
            self.splices.len()
        } else {
            self.splices.iter().position(|(s, _)| *s == serial)?
        };
        Some(self.splices.range(first..).map(|(_, splice)| splice))
    }
    /// The text was replaced wholesale.
    fn new_text_identity(&mut self) {
        self.serial = new_version();
        self.splices.clear();
    }
    /// Append to a read-only inspection document (task output, debugger
    /// console). Not undoable and never makes the document dirty.
    pub fn append_inspection(&mut self, text: &str) {
        let end = self.len();
        self.replace_text(end, end, text);
    }
    /// Replace text in a read-only inspection document (debugger panel).
    pub fn replace_inspection(&mut self, start: usize, end: usize, text: &str) {
        self.replace_text(start, end, text);
    }
    /// An identifier of the current content: equal values mean equal text
    /// (undo back to a state restores its value).
    pub fn content_version(&self) -> u64 {
        self.version
    }
    /// Edits since the last call, for incremental language-server sync.
    /// `None` means too much changed: send the whole document.
    pub fn take_changes(&mut self) -> Option<Vec<TextChange>> {
        self.change_bytes = 0;
        if self.quiet {
            return None;
        }
        self.changes.replace(Vec::new())
    }
    /// Line-level edits since the last call; `None` means "everything".
    pub fn take_line_edits(&mut self) -> Option<Vec<(usize, usize, usize)>> {
        self.line_edits.replace(Vec::new())
    }

    pub fn scratch() -> Self {
        let version = new_version();
        Self {
            path: None,
            read_only: false,
            label: None,
            typing_at: None,
            history_size: 0,
            text: Rope::new(),
            saved: Rope::new(),
            version,
            saved_version: version,
            unavailable: false,
            format: TextFormat::default(),
            saved_format: TextFormat::default(),
            disk: None,
            reference: false,
            recovery_external: false,
            file_read_only: false,
            stamp: None,
            notice: None,
            generation: 0,
            undo: Default::default(),
            undo_steps: 0,
            redo: vec![],
            changes: Some(Vec::new()),
            change_bytes: 0,
            quiet: false,
            line_edits: Some(Vec::new()),
            serial: new_version(),
            splices: Default::default(),
        }
    }
    /// After restoring from a checkpoint: content versions are not stored.
    pub(crate) fn restore_versions(&mut self) {
        self.version = new_version();
        self.saved_version = if self.text == self.saved {
            self.version
        } else {
            new_version()
        };
        self.changes = Some(Vec::new());
        self.line_edits = None;
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
            bail!("Documents are limited to 1 GiB");
        }
        let decoded = text_format::decode(bytes, None)?;
        let mut document = Self::scratch();
        document.text = Rope::from(decoded.text.as_str());
        if !decoded.text.is_empty() {
            // Unsaved input: closing the buffer asks before discarding it.
            document.version = new_version();
        }
        document.format = decoded.format.clone();
        document.saved_format = decoded.format;
        document.notice = Some(
            decoded
                .notice
                .unwrap_or_else(|| format!("Read {} bytes from {label}", bytes.len())),
        );
        Ok(document)
    }
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with_encoding(path, None)
    }
    pub fn open_with_encoding(path: &Path, encoding: Option<&str>) -> Result<Self> {
        let path = path.canonicalize().context("Cannot resolve file")?;
        let (bytes, stamp) = read_regular_bytes(&path)?;
        let disk = Baseline::of(&bytes);
        let decoded = text_format::decode_owned(bytes, encoding).map_err(|(error, _)| error)?;
        Ok(Self::from_disk(path, disk, stamp, decoded))
    }
    /// A document for file contents already read and decoded.
    fn from_disk(
        path: PathBuf,
        disk: Baseline,
        stamp: FileStamp,
        decoded: text_format::Decoded,
    ) -> Self {
        let mut document = Self::scratch();
        document.stamp = Some(stamp);
        document.file_read_only = !fsio::writable(&path);
        // The decoded text is dropped once the rope holds it.
        document.text = Rope::from(decoded.text.as_str());
        drop(decoded.text);
        document.saved = document.text.clone();
        document.format = decoded.format.clone();
        document.saved_format = decoded.format;
        document.disk = Some(disk);
        document.notice = decoded.notice;
        document.path = Some(path);
        document
    }
    pub fn dirty(&self) -> bool {
        !self.read_only
            && (self.unavailable
                || self.version != self.saved_version
                || self.format != self.saved_format)
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
        self.replace_in_group(start, end, value, cursor, 0)
    }
    /// `replace`, recording a revision that undoes with others sharing a
    /// nonzero `group`.
    fn replace_in_group(
        &mut self,
        start: usize,
        end: usize,
        value: &str,
        cursor: usize,
        group: u64,
    ) -> Result<()> {
        self.locked()?;
        self.typing_at = None;
        if start > end
            || end > self.len()
            || !self.is_char_boundary(start)
            || !self.is_char_boundary(end)
        {
            bail!("Invalid text range");
        }
        if self.len() - (end - start) + value.len() > MAX_FILE_BYTES {
            bail!("Documents are limited to 1 GiB");
        }
        if end - start == value.len() && self.slice(start, end) == value {
            return Ok(());
        }
        let version_before = self.version;
        self.version = new_version();
        self.clear_redo();
        let revision = Revision {
            start,
            removed: self.slice(start, end).into_owned(),
            inserted: value.into(),
            before: cursor,
            after: start + value.len(),
            version_before,
            version_after: self.version,
            group,
        };
        self.history_size += revision.removed.len() + revision.inserted.len();
        if starts_step(self.undo.back(), &revision) {
            self.undo_steps += 1;
        }
        self.undo.push_back(revision);
        // Trim whole steps from the front; the newest step is the one being
        // recorded (possibly a group still growing) and always stays.
        while self.undo_steps > 1
            && (self.undo_steps > HISTORY_STEPS
                || self.history_size > HISTORY_BUDGET
                || self.undo.len() > HISTORY_REVISIONS)
        {
            let first = self.undo.pop_front().unwrap();
            self.history_size -= first.removed.len() + first.inserted.len();
            while first.group != 0 && self.undo.front().is_some_and(|r| r.group == first.group) {
                let r = self.undo.pop_front().unwrap();
                self.history_size -= r.removed.len() + r.inserted.len();
            }
            self.undo_steps -= 1;
        }
        self.replace_text(start, end, value);
        Ok(())
    }
    fn clear_redo(&mut self) {
        for r in self.redo.drain(..) {
            self.history_size = self
                .history_size
                .saturating_sub(r.removed.len() + r.inserted.len());
        }
    }
    fn pop_undo(&mut self) -> Option<Revision> {
        let r = self.undo.pop_back()?;
        if starts_step(self.undo.back(), &r) {
            self.undo_steps -= 1;
        }
        Some(r)
    }
    /// Replace several non-overlapping ranges as one undoable edit. Ranges
    /// are in the coordinates of the text before any of them is applied.
    /// Returns where each replacement ended up, in the order given.
    pub fn replace_many(
        &mut self,
        edits: &[(usize, usize, String)],
        cursor: usize,
    ) -> Result<Vec<(usize, usize)>> {
        self.locked()?;
        for (start, end, _) in edits {
            anyhow::ensure!(
                *start <= *end
                    && *end <= self.len()
                    && self.is_char_boundary(*start)
                    && self.is_char_boundary(*end),
                "Invalid text range"
            );
        }
        let mut order: Vec<usize> = (0..edits.len()).collect();
        order.sort_by_key(|i| (edits[*i].0, edits[*i].1));
        for pair in order.windows(2) {
            if edits[pair[0]].1 > edits[pair[1]].0 {
                bail!("Overlapping edits");
            }
        }
        // Positions after all edits: each range shifts by the size change of
        // every earlier range.
        let mut placed = vec![(0, 0); edits.len()];
        let mut shift: isize = 0;
        for &i in &order {
            let (start, end, text) = &edits[i];
            let new_start = (*start as isize + shift) as usize;
            placed[i] = (new_start, new_start + text.len());
            shift += text.len() as isize - (end - start) as isize;
        }
        let group = new_version();
        let mut applied = 0;
        // Apply from the end so earlier offsets stay valid.
        for &i in order.iter().rev() {
            let (start, end, text) = &edits[i];
            let version = self.version;
            if let Err(e) = self.replace_in_group(*start, *end, text, cursor, group) {
                // Roll back what was applied (one undo: they share a group)
                // so the document stays consistent.
                if applied > 0 {
                    self.undo(cursor);
                }
                self.clear_redo();
                return Err(e);
            }
            // A revision is recorded whenever the text changed.
            if self.version != version {
                applied += 1;
            }
        }
        Ok(placed)
    }
    /// The replacements the next undo applies, in order, as
    /// (start, replaced length, inserted length).
    pub fn undo_changes(&self) -> Vec<(usize, usize, usize)> {
        group_changes(self.undo.iter().rev(), |r| {
            (r.start, r.inserted.len(), r.removed.len())
        })
    }
    /// The replacements the next redo applies, in order.
    pub fn redo_changes(&self) -> Vec<(usize, usize, usize)> {
        group_changes(self.redo.iter().rev(), |r| {
            (r.start, r.removed.len(), r.inserted.len())
        })
    }
    pub fn undo(&mut self, cursor: usize) -> Option<usize> {
        if self.read_only {
            return None;
        }
        self.typing_at = None;
        loop {
            let mut r = self.pop_undo()?;
            self.replace_text(r.start, r.start + r.inserted.len(), &r.removed);
            self.version = r.version_before;
            r.after = cursor;
            let position = r.before.min(self.len());
            let group = r.group;
            self.redo.push(r);
            if group == 0 || self.undo.back().is_none_or(|n| n.group != group) {
                return Some(position);
            }
        }
    }
    pub fn redo(&mut self, cursor: usize) -> Option<usize> {
        if self.read_only {
            return None;
        }
        self.typing_at = None;
        loop {
            let mut r = self.redo.pop()?;
            self.replace_text(r.start, r.start + r.removed.len(), &r.inserted);
            self.version = r.version_after;
            r.before = cursor;
            let position = r.after.min(self.len());
            let group = r.group;
            if starts_step(self.undo.back(), &r) {
                self.undo_steps += 1;
            }
            self.undo.push_back(r);
            if group == 0 || self.redo.last().is_none_or(|n| n.group != group) {
                return Some(position);
            }
        }
    }
    /// A detached copy of the content and its save state, without history.
    /// Ropes share structure, so this is cheap even for large documents.
    pub fn checkpoint(&self) -> Self {
        let mut copy = Self::scratch();
        copy.path = self.path.clone();
        copy.read_only = self.read_only;
        copy.label = self.label.clone();
        copy.text = self.text.clone();
        copy.saved = self.saved.clone();
        copy.version = self.version;
        copy.saved_version = self.saved_version;
        copy.unavailable = self.unavailable;
        copy.reference = self.reference;
        copy.recovery_external = self.recovery_external;
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
    pub(crate) fn recovery_ropes(&self) -> (&Rope, &Rope) {
        (&self.text, &self.saved)
    }
    pub(crate) fn set_recovery_ropes(&mut self, text: Rope, saved: Rope) {
        self.text = text;
        self.saved = saved;
        self.new_text_identity();
    }
    pub fn accept_save(&mut self, saved: Self) {
        self.unavailable = false;
        self.typing_at = None;
        self.path = saved.path;
        self.saved = saved.saved;
        self.saved_version = saved.saved_version;
        self.saved_format = saved.saved_format;
        self.disk = saved.disk;
        self.stamp = saved.stamp;
        self.file_read_only = saved.file_read_only;
        self.generation += 1;
    }
    pub fn mark_unavailable(&mut self) {
        self.unavailable = true;
    }
    pub fn valid_recovery(&self) -> bool {
        !self.recovery_external
            && self.text.len_bytes() <= MAX_FILE_BYTES
            && self.saved.len_bytes() <= MAX_FILE_BYTES
    }
    pub fn discard_changes(&mut self) {
        self.unavailable = false;
        self.text = self.saved.clone();
        self.new_text_identity();
        self.version = self.saved_version;
        self.format = self.saved_format.clone();
        self.generation += 1;
        self.undo.clear();
        self.undo_steps = 0;
        self.redo.clear();
        self.history_size = 0;
        self.typing_at = None;
        self.changes = None;
        self.line_edits = None;
    }
    pub fn history_bytes(&self) -> usize {
        self.history_size
    }
    /// Change how the document is stored. The change is saved like an edit.
    pub fn set_format(&mut self, format: TextFormat) -> Result<()> {
        self.locked()?;
        text_format::validate_rope(&self.text, &format)?;
        self.format = format;
        self.generation += 1;
        Ok(())
    }
    /// The bytes a save would write.
    pub fn encoded(&self) -> Result<Vec<u8>> {
        text_format::encode_rope(&self.text, &self.format)
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
        self.prepare_save(destination, overwrite)?;
        self.write_prepared(options)
    }
    /// Resolve the destination and its expected contents once, before retries.
    pub(crate) fn prepare_save(
        &mut self,
        destination: Option<&Path>,
        overwrite: bool,
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
            Some(Baseline::of(&read_regular_bytes(&path)?.0))
        } else {
            None
        };
        self.path = Some(path);
        self.disk = baseline;
        Ok(())
    }
    pub(crate) fn write_prepared(&mut self, options: WriteOptions) -> Result<()> {
        let path = self
            .path
            .clone()
            .context("Use Save As for an untitled document")?;
        let bytes = self.encoded()?;
        let written = fsio::write_file(&path, &bytes, self.disk.as_ref(), options)?;
        self.disk = Some(written);
        self.path = Some(path);
        self.saved = self.text.clone();
        self.saved_version = self.version;
        self.saved_format = self.format.clone();
        self.stamp = self.path.as_deref().and_then(FileStamp::of);
        self.file_read_only = self.path.as_deref().is_some_and(|p| !fsio::writable(p));
        self.unavailable = false;
        Ok(())
    }
    /// Record a save performed by an elevated helper.
    pub(crate) fn mark_saved_bytes(&mut self, bytes: &[u8]) {
        self.disk = Some(Baseline::of(bytes));
        self.stamp = self.path.as_deref().and_then(FileStamp::of);
        self.saved = self.text.clone();
        self.saved_version = self.version;
        self.saved_format = self.format.clone();
        self.unavailable = false;
    }
    pub(crate) fn unavailable_flag(&self) -> bool {
        self.unavailable
    }
    /// Keep this buffer but treat the current disk version as its baseline,
    /// so the next save replaces it deliberately.
    pub(crate) fn adopt_disk_baseline(&mut self, fresh: &Document) {
        self.disk = fresh.disk.clone();
        self.stamp = fresh.stamp;
        self.unavailable = false;
    }
    pub fn baseline_matches(&self, bytes: &[u8]) -> bool {
        self.disk.as_ref() == Some(&Baseline::of(bytes))
    }
}

// ----- Rope helpers -----

/// A regular file's contents (never blocking on a FIFO or reading a device)
/// and the stamp of exactly the file read.
fn read_regular_bytes(path: &Path) -> Result<(Vec<u8>, FileStamp)> {
    // A replaced path must not block on a FIFO between stat and open.
    let (file, meta) = crate::fsio::open_regular(path)?;
    if meta.len() > MAX_FILE_BYTES as u64 {
        bail!("Documents are limited to 1 GiB");
    }
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(file, (MAX_FILE_BYTES + 1) as u64),
        &mut bytes,
    )?;
    if bytes.len() > MAX_FILE_BYTES {
        bail!("Documents are limited to 1 GiB");
    }
    Ok((bytes, FileStamp::of_metadata(&meta)))
}

/// Plain ASCII without tabs or carriage returns: one column per byte. Uses
/// byte searches (memchr) so it stays fast even in unoptimised builds.
fn plain(chunk: &str) -> bool {
    let bytes = chunk.as_bytes();
    chunk.is_ascii() && !bytes.contains(&b'\t') && !bytes.contains(&b'\r')
}
fn advance(chunk: &str, mut col: usize, tab: usize) -> usize {
    if plain(chunk) {
        return col + chunk.len();
    }
    for g in chunk.graphemes(true) {
        col += grapheme_width(g, col, tab);
    }
    col
}
/// Display width of a rope slice measured from the start of its line.
pub fn slice_width(slice: RopeSlice, tab: usize) -> usize {
    let tab = tab.max(1);
    slice
        .chunks()
        .fold(0, |col, chunk| advance(chunk, col, tab))
}
/// Byte offset within a line slice closest to a display column.
pub fn slice_column_offset(line: RopeSlice, col: usize, tab: usize) -> usize {
    let tab = tab.max(1);
    let mut x = 0;
    let mut base = 0;
    for chunk in line.chunks() {
        if plain(chunk) {
            if x + chunk.len() > col {
                return base + (col - x);
            }
            x += chunk.len();
        } else {
            for (i, g) in chunk.grapheme_indices(true) {
                if g == "\r" || g == "\r\n" {
                    return base + i;
                }
                let w = grapheme_width(g, x, tab);
                if x + w > col {
                    return base + i;
                }
                x += w;
            }
        }
        base += chunk.len();
    }
    line.len_bytes()
}
fn next_grapheme(slice: &RopeSlice, offset: usize) -> usize {
    if offset >= slice.len_bytes() {
        return slice.len_bytes();
    }
    let (mut chunk, mut chunk_start, _, _) = slice.chunk_at_byte(offset);
    let mut cursor = GraphemeCursor::new(offset, slice.len_bytes(), true);
    loop {
        match cursor.next_boundary(chunk, chunk_start) {
            Ok(None) => return slice.len_bytes(),
            Ok(Some(n)) => return n,
            Err(GraphemeIncomplete::NextChunk) => {
                chunk_start += chunk.len();
                chunk = slice.chunk_at_byte(chunk_start).0;
            }
            Err(GraphemeIncomplete::PreContext(n)) => {
                let context = slice.chunk_at_byte(n - 1).0;
                cursor.provide_context(context, n - context.len());
            }
            Err(_) => return (offset + 1).min(slice.len_bytes()),
        }
    }
}
fn previous_grapheme(slice: &RopeSlice, offset: usize) -> usize {
    if offset == 0 {
        return 0;
    }
    let (mut chunk, mut chunk_start, _, _) = slice.chunk_at_byte(offset);
    let mut cursor = GraphemeCursor::new(offset, slice.len_bytes(), true);
    loop {
        match cursor.prev_boundary(chunk, chunk_start) {
            Ok(None) => return 0,
            Ok(Some(n)) => return n,
            Err(GraphemeIncomplete::PrevChunk) => {
                let (previous, start, _, _) = slice.chunk_at_byte(chunk_start - 1);
                chunk = previous;
                chunk_start = start;
            }
            Err(GraphemeIncomplete::PreContext(n)) => {
                let context = slice.chunk_at_byte(n - 1).0;
                cursor.provide_context(context, n - context.len());
            }
            Err(_) => return offset - 1,
        }
    }
}

/// Cheap identity of a file's current version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileStamp {
    len: u64,
    modified: Option<std::time::SystemTime>,
    inode: u64,
}
impl FileStamp {
    pub fn of(path: &Path) -> Option<Self> {
        Some(Self::of_metadata(&fs::metadata(path).ok()?))
    }
    fn of_metadata(meta: &fs::Metadata) -> Self {
        #[cfg(unix)]
        let inode = std::os::unix::fs::MetadataExt::ino(meta);
        #[cfg(not(unix))]
        let inode = 0;
        Self {
            len: meta.len(),
            modified: meta.modified().ok(),
            inode,
        }
    }
}

/// What the change watcher found for one document.
pub enum DiskChange {
    /// Only the stamp moved (e.g. `touch`); the content is unchanged.
    Touched(Option<FileStamp>),
    /// The content differs from the document's baseline.
    Changed(Box<Document>),
    Missing,
    /// The file cannot be read now (permissions, or it is mid-write).
    Unreadable,
}
impl Document {
    /// A snapshot of what the watcher needs, taken on the UI thread.
    pub(crate) fn watch_entry(
        &self,
    ) -> Option<(PathBuf, Option<Baseline>, Option<FileStamp>, String)> {
        if self.label.is_some() || self.reference {
            return None;
        }
        let path = self.path.clone()?;
        // A file that has not been created yet has nothing to watch.
        if self.disk.is_none() && self.stamp.is_none() {
            return None;
        }
        Some((
            path,
            self.disk.clone(),
            self.stamp,
            self.format.encoding.clone(),
        ))
    }
}
/// Compare a file with its document baseline (runs on the IO worker).
pub(crate) fn examine(
    path: &Path,
    baseline: Option<&Baseline>,
    stamp: Option<FileStamp>,
    encoding: &str,
) -> Option<DiskChange> {
    let current = FileStamp::of(path);
    if current.is_none() {
        return (baseline.is_some()).then_some(DiskChange::Missing);
    }
    if current == stamp {
        return None;
    }
    // Read once, never blocking on a FIFO or reading past the size limit
    // (the IO worker is shared), and decode those same bytes.
    let Ok(path) = path.canonicalize() else {
        return Some(DiskChange::Unreadable);
    };
    let Ok((bytes, opened)) = read_regular_bytes(&path) else {
        return Some(DiskChange::Unreadable);
    };
    let disk = Baseline::of(&bytes);
    if baseline == Some(&disk) {
        return Some(DiskChange::Touched(Some(opened)));
    }
    let decoded = text_format::decode_owned(bytes, Some(encoding))
        .or_else(|(_, bytes)| text_format::decode_owned(bytes, None));
    match decoded {
        Ok(decoded) => Some(DiskChange::Changed(Box::new(Document::from_disk(
            path, disk, opened, decoded,
        )))),
        Err(_) => Some(DiskChange::Unreadable),
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
        let tab = tab.max(1);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text_format::LineEnding;

    #[cfg(unix)]
    #[test]
    fn examining_a_fifo_or_oversized_file_returns_promptly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        fs::write(&path, "abc").unwrap();
        let d = Document::open(&path).unwrap();
        let (_, baseline, stamp, encoding) = d.watch_entry().unwrap();
        // The file is replaced by a FIFO nobody writes to.
        fs::remove_file(&path).unwrap();
        let fifo = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let started = std::time::Instant::now();
        let change = examine(&path, baseline.as_ref(), stamp, &encoding);
        assert!(matches!(change, Some(DiskChange::Unreadable)));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        // A (sparse) file past the size limit is not read.
        fs::remove_file(&path).unwrap();
        let file = fs::File::create(&path).unwrap();
        file.set_len(MAX_FILE_BYTES as u64 + 1).unwrap();
        let change = examine(&path, baseline.as_ref(), stamp, &encoding);
        assert!(matches!(change, Some(DiskChange::Unreadable)));
        // Real changes decode the bytes read, with the stamp of that file.
        fs::write(&path, b"caf\xe9\r\n").unwrap();
        let Some(DiskChange::Changed(fresh)) = examine(&path, baseline.as_ref(), stamp, &encoding)
        else {
            panic!("the change was not seen");
        };
        assert_eq!(fresh.text(), "café\n");
        assert_eq!(fresh.format.encoding, "windows-1252");
        assert_eq!(fresh.stamp, FileStamp::of(&path));
        assert!(fresh.baseline_matches(b"caf\xe9\r\n"));
    }

    #[test]
    fn decoding_reuses_the_buffer_and_keeps_formats() {
        for (bytes, text, encoding, bom, ending) in [
            (&b"plain\n"[..], "plain\n", "UTF-8", false, LineEnding::Lf),
            (
                b"\xEF\xBB\xBFa\r\nb\r\n",
                "a\nb\n",
                "UTF-8",
                true,
                LineEnding::Crlf,
            ),
            (
                b"a\rb\r\xc3\xa9",
                "a\nb\n\u{e9}",
                "UTF-8",
                false,
                LineEnding::Cr,
            ),
            (
                b"caf\xe9\r\n",
                "caf\u{e9}\n",
                "windows-1252",
                false,
                LineEnding::Crlf,
            ),
            (
                b"\xFF\xFEa\0\r\0\n\0",
                "a\n",
                "UTF-16LE",
                true,
                LineEnding::Crlf,
            ),
        ] {
            let decoded = text_format::decode_owned(bytes.to_vec(), None).unwrap();
            assert_eq!(decoded.text, text);
            assert_eq!(
                (decoded.format.encoding.as_str(), decoded.format.bom),
                (encoding, bom)
            );
            assert_eq!(decoded.format.line_ending, ending);
        }
        // A failure hands the bytes back unchanged, BOM included.
        let bytes = b"\xEF\xBB\xBF\xff".to_vec();
        let Err((_, back)) = text_format::decode_owned(bytes.clone(), None) else {
            panic!("invalid UTF-8 after a BOM decoded");
        };
        assert_eq!(back, bytes);
        let Err((_, back)) = text_format::decode_owned(b"\xff".to_vec(), Some("utf-8")) else {
            panic!("invalid forced UTF-8 decoded");
        };
        assert_eq!(back, b"\xff");
    }

    #[test]
    fn zero_tab_width_never_divides_by_zero() {
        assert_eq!(grapheme_width("\t", 3, 0), 1);
        assert_eq!(display_width_with_tabs("a\tb", 0), 3);
        assert_eq!(column_offset("\tb", 1, 0), 1);
    }

    #[test]
    fn grouped_edits_undo_together_even_with_full_history() {
        let mut d = Document::from_text("abc".into()).unwrap();
        // Fill the history past its 10,000-revision limit.
        for i in 0..10_050 {
            let len = d.len();
            d.replace(len, len, if i % 2 == 0 { "x" } else { "y" }, 0)
                .unwrap();
        }
        let text = d.text();
        d.replace_many(
            &[(0, 1, "A".into()), (1, 2, "B".into()), (2, 3, "C".into())],
            0,
        )
        .unwrap();
        assert!(d.text().starts_with("ABC"));
        d.undo(0);
        assert_eq!(d.text(), text, "one undo reverts every part");
        d.redo(0);
        assert!(d.text().starts_with("ABC"));
        // A failing part rolls back the parts already applied.
        let before = d.text();
        assert!(d
            .replace_many(
                &[(0, 1, "1".into()), (d.len() + 5, d.len() + 6, "2".into())],
                0
            )
            .is_err());
        assert_eq!(d.text(), before);
    }

    #[test]
    fn a_group_larger_than_the_history_budget_undoes_completely() {
        let mut d = Document::from_text("ab".into()).unwrap();
        d.replace(0, 0, "x", 0).unwrap();
        let part = "y".repeat(HISTORY_BUDGET / 2 + 1);
        d.replace_many(&[(1, 1, part.clone()), (2, 2, part)], 0)
            .unwrap();
        assert_eq!(d.undo_steps, 1, "older steps make room; the group stays");
        d.undo(0);
        assert_eq!(d.text(), "xab");
        assert!(d.undo(0).is_none());
        d.redo(0);
        assert_eq!(d.len(), 3 + HISTORY_BUDGET + 2);
    }

    #[test]
    fn history_limit_counts_undo_steps_not_revisions() {
        let mut d = Document::from_text("a".repeat(HISTORY_STEPS + 1)).unwrap();
        for _ in 0..5 {
            d.replace(0, 0, "-", 0).unwrap();
        }
        let original = d.text();
        // One caret per character: one step of more than 10,000 revisions.
        let edits: Vec<_> = (5..d.len()).map(|i| (i, i + 1, "b".into())).collect();
        d.replace_many(&edits, 0).unwrap();
        d.undo(0);
        assert_eq!(d.text(), original);
        for _ in 0..5 {
            d.undo(0).unwrap();
        }
        assert_eq!(d.text(), "a".repeat(HISTORY_STEPS + 1));
        assert!(d.undo(0).is_none());
        // Interleaved undo and redo keep the step count exact.
        for _ in 0..6 {
            d.redo(0).unwrap();
        }
        assert_eq!(d.undo_steps, 6);
        assert!(d.text().ends_with(&"b".repeat(HISTORY_STEPS + 1)));
        // Past the limit whole steps go, oldest first.
        for i in 0..HISTORY_STEPS {
            d.replace(i % 3, i % 3, "z", 0).unwrap();
        }
        assert_eq!(d.undo_steps, HISTORY_STEPS);
        assert!(d.undo.iter().all(|r| r.group == 0));
    }
    #[test]
    fn history_revisions_are_capped_a_whole_step_at_a_time() {
        let mut d = Document::from_text("a".repeat(10_000)).unwrap();
        let steps = HISTORY_REVISIONS / 10_000 + 5;
        for step in 0..steps {
            let value = if step % 2 == 0 { "b" } else { "a" };
            let edits: Vec<_> = (0..d.len()).map(|i| (i, i + 1, value.into())).collect();
            d.replace_many(&edits, 0).unwrap();
        }
        assert!(d.undo.len() <= HISTORY_REVISIONS);
        assert_eq!(d.undo.len() % 10_000, 0, "steps are trimmed whole");
        assert_eq!(d.undo_steps, d.undo.len() / 10_000);
        // Every kept step still undoes completely.
        while d.undo(0).is_some() {
            assert!(d.text().chars().all(|c| c == 'a') || d.text().chars().all(|c| c == 'b'));
        }
    }
}
