use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, io::Write, path::PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StartupMode {
    EditorOnly,
    Workspace,
}

impl std::str::FromStr for StartupMode {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        match value {
            "editor-only" => Ok(Self::EditorOnly),
            "workspace" => Ok(Self::Workspace),
            _ => bail!("Startup mode must be editor-only or workspace"),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub indent_width: usize,
    pub insert_spaces: bool,
    pub auto_indent: bool,
    pub line_numbers: bool,
    pub theme: String,
    pub file_startup: StartupMode,
    pub directory_startup: StartupMode,
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
            file_startup: StartupMode::EditorOnly,
            directory_startup: StartupMode::Workspace,
            global_keys: keys(&[
                ("f6", "next-pane"),
                ("f7", "next-tab"),
                ("f8", "terminal"),
                ("f9", "split-right"),
                ("Shift+f9", "split-down"),
                ("f10", "toggle-workspace"),
                ("Ctrl+,", "settings"),
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
    pub fn menu_items(&self) -> Vec<(&'static str, String)> {
        let mode = |value| match value {
            StartupMode::EditorOnly => "Editor only",
            StartupMode::Workspace => "Full workspace",
        };
        vec![
            ("Opening a file", mode(self.file_startup).into()),
            ("Opening a directory", mode(self.directory_startup).into()),
            ("Indent width", self.indent_width.to_string()),
            ("Insert spaces", self.insert_spaces.to_string()),
            ("Auto indent", self.auto_indent.to_string()),
            ("Line numbers", self.line_numbers.to_string()),
            ("Theme", self.theme.clone()),
        ]
    }
    pub fn path() -> PathBuf {
        crate::paths::config_dir().join("settings.toml")
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
        let mut settings: Self =
            toml::from_str(&fs::read_to_string(path)?).context("Invalid settings.toml")?;
        // Settings saved by older releases contain the old complete key map.
        // Add the new defaults while preserving explicit user assignments.
        for (key, action) in [("f10", "toggle-workspace"), ("Ctrl+,", "settings")] {
            settings
                .global_keys
                .entry(key.into())
                .or_insert_with(|| action.into());
        }
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
    pub(super) fn settings_key(&mut self, key: &crate::Key) -> Result<()> {
        let count = self.preferences.menu_items().len();
        let prompt = self.prompt.as_mut().unwrap();
        match key.key.as_str() {
            "Escape" => self.prompt = None,
            "Up" => prompt.field = (prompt.field + count - 1) % count,
            "Down" | "Tab" => prompt.field = (prompt.field + 1) % count,
            "Left" | "Right" | "Enter" | "Space" | " " => {
                let backwards = key.key == "Left";
                let toggle_mode = |mode| {
                    if mode == StartupMode::EditorOnly {
                        "workspace"
                    } else {
                        "editor-only"
                    }
                };
                let (name, value) = match prompt.field {
                    0 => (
                        "file-startup",
                        toggle_mode(self.preferences.file_startup).into(),
                    ),
                    1 => (
                        "directory-startup",
                        toggle_mode(self.preferences.directory_startup).into(),
                    ),
                    2 => (
                        "indent-width",
                        (if backwards {
                            self.preferences.indent_width.saturating_sub(1).max(1)
                        } else {
                            (self.preferences.indent_width + 1).min(16)
                        })
                        .to_string(),
                    ),
                    3 => (
                        "insert-spaces",
                        (!self.preferences.insert_spaces).to_string(),
                    ),
                    4 => ("auto-indent", (!self.preferences.auto_indent).to_string()),
                    5 => ("line-numbers", (!self.preferences.line_numbers).to_string()),
                    _ => {
                        let themes = ["auto", "dark", "light"];
                        let current = themes
                            .iter()
                            .position(|t| *t == self.preferences.theme)
                            .unwrap_or(0);
                        (
                            "theme",
                            themes[(current + if backwards { 2 } else { 1 }) % 3].into(),
                        )
                    }
                };
                self.configure(name, &value)?;
            }
            _ => {}
        }
        Ok(())
    }
    pub(super) fn load_preferences(&mut self) {
        match Preferences::load() {
            Ok(p) => self.preferences = p,
            Err(e) => self.status = format!("Settings error: {e:#}"),
        }
        if let ("auto", Some(palette)) =
            (self.preferences.theme.as_str(), self.system_colors.as_ref())
        {
            self.colors = (
                palette.0.clone(),
                palette.1.clone(),
                palette.2.clone(),
                palette.3.clone(),
            );
            self.selection_foreground = palette.4.clone();
        } else if self.preferences.theme == "light" {
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
            "file-startup" => settings.file_startup = value.parse()?,
            "directory-startup" => settings.directory_startup = value.parse()?,
            _ => bail!("Options: indent-width, insert-spaces, auto-indent, line-numbers, theme, file-startup, directory-startup"),
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
