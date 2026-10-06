use anyhow::{bail, Context, Result};
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
    text: String,
    cursor: usize,
}

pub struct Document {
    pub path: Option<PathBuf>,
    pub text: String,
    saved: String,
    disk: Option<Vec<u8>>,
    undo: Vec<Revision>,
    redo: Vec<Revision>,
}

impl Document {
    pub fn scratch() -> Self {
        Self {
            path: None,
            text: String::new(),
            saved: String::new(),
            disk: None,
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
        let text =
            String::from_utf8(bytes.clone()).context("Slate currently edits UTF-8 text only")?;
        Ok(Self {
            path: Some(path),
            saved: text.clone(),
            text,
            disk: Some(bytes),
            undo: vec![],
            redo: vec![],
        })
    }
    pub fn dirty(&self) -> bool {
        self.text != self.saved
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
        self.undo.push(Revision {
            text: self.text.clone(),
            cursor,
        });
        let mut total: usize = self.undo.iter().map(|r| r.text.len()).sum();
        while self.undo.len() > 1 && (self.undo.len() > 256 || total > HISTORY_BUDGET) {
            total -= self.undo.remove(0).text.len();
        }
        self.redo.clear();
        self.text.replace_range(start..end, value);
        Ok(())
    }
    pub fn undo(&mut self, cursor: usize) -> Option<usize> {
        let r = self.undo.pop()?;
        self.redo.push(Revision {
            text: std::mem::replace(&mut self.text, r.text),
            cursor,
        });
        Some(r.cursor.min(self.text.len()))
    }
    pub fn redo(&mut self, cursor: usize) -> Option<usize> {
        let r = self.redo.pop()?;
        self.undo.push(Revision {
            text: std::mem::replace(&mut self.text, r.text),
            cursor,
        });
        Some(r.cursor.min(self.text.len()))
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
            if self.disk.as_deref() != Some(fs::read(&path)?.as_slice()) {
                bail!("File changed on disk. Save As to a new path or reopen after preserving your changes");
            }
        } else if same && self.disk.is_some() {
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
        if same && path.exists() && self.disk.as_deref() != Some(fs::read(&path)?.as_slice()) {
            bail!("File changed during save");
        }
        if same {
            temp.persist(&path).map_err(|e| e.error)?;
        } else {
            temp.persist_noclobber(&path).map_err(|e| e.error)?;
        }
        self.path = Some(path);
        self.saved = self.text.clone();
        self.disk = Some(self.text.as_bytes().to_vec());
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
    let start = line_start(text, cursor);
    (
        text[..start].bytes().filter(|b| *b == b'\n').count(),
        display_width(&text[start..cursor]),
    )
}
pub fn display_width(text: &str) -> usize {
    let mut col = 0;
    for g in text.graphemes(true) {
        col += if g == "\t" {
            4 - col % 4
        } else {
            UnicodeWidthStr::width(g)
        };
    }
    col
}
pub fn at_line_col(text: &str, row: usize, col: usize) -> usize {
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
            4 - x % 4
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
