use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, io::Write, path::PathBuf};

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub indent_width: usize,
    pub insert_spaces: bool,
    pub auto_indent: bool,
    pub line_numbers: bool,
    pub theme: String,
    pub global_keys: BTreeMap<String, String>,
    pub editor_keys: BTreeMap<String, String>,
    pub terminal_keys: BTreeMap<String, String>,
}
impl Default for Preferences {
    fn default() -> Self {
        let keys = |entries: &[(&str, &str)]| {
            entries
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect()
        };
        Self {
            indent_width: 4,
            insert_spaces: true,
            auto_indent: true,
            line_numbers: true,
            theme: "auto".into(),
            global_keys: keys(&[
                ("f6", "next-pane"),
                ("f7", "next-tab"),
                ("f8", "terminal"),
                ("f9", "split-right"),
                ("Shift+f9", "split-down"),
            ]),
            editor_keys: keys(&[
                ("Ctrl+s", "save"),
                ("Ctrl+z", "undo"),
                ("Ctrl+Shift+z", "redo"),
                ("Ctrl+y", "redo"),
                ("Ctrl+a", "select-all"),
                ("Ctrl+c", "copy"),
                ("Ctrl+x", "cut"),
                ("Ctrl+v", "paste"),
                ("Ctrl+f", "prompt-find"),
                ("Ctrl+h", "prompt-replace"),
                ("Alt+r", "prompt-replace"),
                ("Ctrl+g", "prompt-goto"),
                ("f3", "find-next"),
                ("Shift+f3", "find-previous"),
                ("Ctrl+]", "indent"),
                ("Ctrl+[", "outdent"),
            ]),
            terminal_keys: keys(&[
                ("Ctrl+Shift+c", "copy"),
                ("Ctrl+Shift+v", "paste"),
                ("Alt+c", "copy"),
                ("Alt+v", "paste"),
            ]),
        }
    }
}
impl Preferences {
    pub fn path() -> PathBuf {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
            })
            .join("slate/settings.toml")
    }
    pub fn validate(&self) -> Result<()> {
        if !(1..=16).contains(&self.indent_width) {
            bail!("indent_width must be 1–16");
        }
        if !["auto", "dark", "light"].contains(&self.theme.as_str()) {
            bail!("theme must be auto, dark or light");
        }
        if self.global_keys.len() + self.editor_keys.len() + self.terminal_keys.len() > 256 {
            bail!("Too many key bindings");
        }
        Ok(())
    }
    pub fn load() -> Result<Self> {
        let path = Self::path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let settings: Self =
            toml::from_str(&fs::read_to_string(path)?).context("Invalid settings.toml")?;
        settings.validate()?;
        Ok(settings)
    }
    pub fn save(&self) -> Result<()> {
        self.validate()?;
        let path = Self::path();
        fs::create_dir_all(path.parent().unwrap())?;
        let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
        file.write_all(toml::to_string_pretty(self)?.as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(path).map_err(|e| e.error)?;
        Ok(())
    }
}

use crate::App;

impl App {
    pub(super) fn load_preferences(&mut self) {
        match Preferences::load() {
            Ok(p) => self.preferences = p,
            Err(e) => self.status = format!("Settings error: {e:#}"),
        }
        if self.preferences.theme == "light" {
            self.selection_foreground = "#20242c".into();
            self.colors = (
                "#20242c".into(),
                "#ffffff".into(),
                "#bdd9f5".into(),
                "#1769aa".into(),
            );
        } else {
            self.selection_foreground = "#ffffff".into();
            self.colors = (
                "#d8dee9".into(),
                "#20242c".into(),
                "#425b78".into(),
                "#88c0d0".into(),
            );
        }
        self.tokens.clear();
    }
    pub(super) fn configure(&mut self, name: &str, value: &str) -> Result<()> {
        let mut settings = self.preferences.clone();
        match name {
            "indent-width" => settings.indent_width = value.parse()?,
            "insert-spaces" => settings.insert_spaces = value.parse()?,
            "auto-indent" => settings.auto_indent = value.parse()?,
            "line-numbers" => settings.line_numbers = value.parse()?,
            "theme" => settings.theme = value.into(),
            _ => bail!("Options: indent-width, insert-spaces, auto-indent, line-numbers, theme"),
        }
        settings.save()?;
        self.load_preferences();
        self.status = format!("Set {name} to {value}");
        Ok(())
    }
    pub(super) fn light_theme(&self) -> bool {
        let color = u32::from_str_radix(&self.colors.1[1..], 16).unwrap_or(0);
        ((color >> 16) & 255) * 299 + ((color >> 8) & 255) * 587 + (color & 255) * 114 > 128_000
    }
}
