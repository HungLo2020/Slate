//! Standards-compliant EditorConfig defaults, cached until settings reload.
use crate::{profiles::EditorOptions, App};
use anyhow::{bail, Context, Result};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Mutex,
};

#[derive(Clone, Default)]
pub(crate) struct Settings {
    pub tab: Option<usize>,
    pub indent: Option<usize>,
    pub spaces: Option<bool>,
    pub ending: Option<String>,
    pub charset: Option<String>,
    pub trim: bool,
    pub final_newline: bool,
}
#[derive(Default)]
pub(crate) struct Cache(Mutex<BTreeMap<PathBuf, Result<Settings, String>>>);
impl Cache {
    pub(crate) fn get(&self, path: &Path) -> Result<Settings, String> {
        self.0
            .lock()
            .unwrap()
            .entry(path.to_path_buf())
            .or_insert_with(|| load(path).map_err(|e| format!("{e:#}")))
            .clone()
    }
}
fn load(path: &Path) -> Result<Settings> {
    for parent in path.ancestors().skip(1) {
        let config = parent.join(".editorconfig");
        if let Ok(meta) = std::fs::metadata(&config) {
            if !meta.is_file() || meta.len() > 128 * 1024 {
                bail!("{} must be a regular file below 128 KiB", config.display());
            }
        }
    }
    let mut properties = ec4rs::properties_of(path).context("Cannot read EditorConfig")?;
    properties.use_fallbacks();
    let raw = |name| {
        properties
            .get_raw_for_key(name)
            .filter_unset()
            .into_option()
    };
    let spaces = match raw("indent_style") {
        Some("space") => Some(true),
        Some("tab") => Some(false),
        _ => None,
    };
    let indent = raw("indent_size")
        .and_then(|n| n.parse::<usize>().ok())
        .filter(|n| (1..=32).contains(n));
    let tab = raw("tab_width")
        .and_then(|n| n.parse::<usize>().ok())
        .filter(|n| (1..=32).contains(n));
    Ok(Settings {
        tab,
        indent,
        spaces,
        ending: raw("end_of_line")
            .filter(|s| ["lf", "crlf", "cr"].contains(s))
            .map(str::to_owned),
        charset: raw("charset")
            .filter(|s| ["utf-8", "utf-8-bom", "utf-16le", "utf-16be", "latin1"].contains(s))
            .map(str::to_owned),
        trim: raw("trim_trailing_whitespace") == Some("true"),
        final_newline: raw("insert_final_newline") == Some("true"),
    })
}
impl Settings {
    pub(crate) fn edits(&self, rope: &ropey::Rope) -> Result<Vec<(usize, usize, String)>> {
        let mut edits = Vec::new();
        let mut offset = 0;
        if self.trim {
            for line in rope.lines() {
                let len = line.len_bytes();
                let mut end = line.len_chars();
                if end > 0 && line.char(end - 1) == '\n' {
                    end -= 1;
                }
                let original = line.char_to_byte(end);
                while end > 0 && matches!(line.char(end - 1), ' ' | '\t') {
                    end -= 1;
                }
                let trimmed = line.char_to_byte(end);
                if trimmed < original {
                    edits.push((offset + trimmed, offset + original, String::new()));
                }
                offset += len;
                if edits.len() > 10_000 {
                    bail!("EditorConfig trimming exceeds 10,000 lines; disable trim_trailing_whitespace for this file");
                }
            }
        }
        if self.final_newline && rope.len_chars() > 0 && rope.char(rope.len_chars() - 1) != '\n' {
            edits.push((rope.len_bytes(), rope.len_bytes(), "\n".into()));
        }
        Ok(edits)
    }
    pub(crate) fn apply(&self, options: &mut EditorOptions) {
        if let Some(n) = self.tab {
            options.tab_width = n;
        }
        if let Some(n) = self.indent {
            options.indent_width = n;
        }
        if let Some(b) = self.spaces {
            options.insert_spaces = b;
        }
    }
}
impl App {
    pub(crate) fn editorconfig_settings(
        &self,
        doc: u64,
        destination: Option<&Path>,
    ) -> Result<Settings> {
        let path = destination
            .map(Path::to_path_buf)
            .or_else(|| self.documents[&doc].path.clone());
        let Some(path) = path else {
            return Ok(Settings::default());
        };
        let path = crate::document::absolute(&path)?;
        self.preference_layers
            .editorconfig
            .get(&path)
            .map_err(anyhow::Error::msg)
    }
}
impl Settings {
    pub(crate) fn format(
        &self,
        mut format: crate::text_format::TextFormat,
    ) -> Result<crate::text_format::TextFormat> {
        if let Some(ending) = &self.ending {
            format.line_ending = ending.parse()?;
        }
        if let Some(charset) = &self.charset {
            format.bom = charset == "utf-8-bom" || charset.starts_with("utf-16");
            format.encoding = crate::text_format::encoding_name(match charset.as_str() {
                "utf-8-bom" => "utf-8",
                s => s,
            })?;
        }
        Ok(format)
    }
}
