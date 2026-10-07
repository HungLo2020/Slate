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
    /// `default` or `nano`. Choosing a keymap replaces the key tables below.
    pub keymap: String,
    /// Wrap long lines at the window edge instead of scrolling horizontally.
    pub soft_wrap: bool,
    /// Column used by justify and hard wrapping.
    pub wrap_column: usize,
    /// Break lines automatically while typing past `wrap_column`.
    pub hard_wrap: bool,
    /// Keep the previous file contents as `NAME~` when saving.
    pub backup: bool,
    /// Restore unsaved buffers of individually opened files after a crash.
    /// Directory workspaces always keep recovery checkpoints.
    pub file_recovery: bool,
    /// Capture the mouse in the terminal interface. Off leaves selection to
    /// the outer terminal.
    pub tui_mouse: bool,
    /// Mark spaces (·) and tabs (→) in editors.
    pub show_whitespace: bool,
    /// Reload unmodified documents when their file changes on disk.
    pub auto_reload: bool,
    /// Show the document overview beside GUI editors.
    pub minimap: bool,
    /// GUI editor font; empty uses the desktop's fixed-width font.
    pub font_family: String,
    /// GUI editor font size in points.
    pub font_size: usize,
    /// Lines of history kept by each terminal.
    pub terminal_scrollback: usize,
    /// Typing an opening bracket or quote inserts its closer.
    pub auto_close_brackets: bool,
    /// Format documents before saving them (trusted workspaces only).
    pub format_on_save: bool,
    pub global_keys: BTreeMap<String, String>,
    pub editor_keys: BTreeMap<String, String>,
    pub terminal_keys: BTreeMap<String, String>,
}

fn keys(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}
const TERMINAL_KEYS: &[(&str, &str)] = &[
    ("Ctrl+Shift+c", "copy"),
    ("Ctrl+Shift+v", "paste"),
    ("Alt+c", "copy"),
    ("Alt+v", "paste"),
];
const COMMON_GLOBAL_KEYS: &[(&str, &str)] = &[
    ("f6", "next-pane"),
    ("f7", "next-tab"),
    ("f8", "terminal"),
    ("f9", "split-right"),
    ("Shift+f9", "split-down"),
    ("f10", "toggle-workspace"),
    ("Ctrl+,", "settings"),
];

/// Key tables for a named keymap: (global, editor, terminal).
#[allow(clippy::type_complexity)]
pub fn keymap(
    name: &str,
) -> Result<(
    BTreeMap<String, String>,
    BTreeMap<String, String>,
    BTreeMap<String, String>,
)> {
    let mut global = keys(COMMON_GLOBAL_KEYS);
    let editor = match name {
        "default" => {
            global.extend(keys(&[
                ("Ctrl+q", "quit"),
                ("Ctrl+o", "open"),
                ("Ctrl+Shift+o", "open-folder"),
            ]));
            keys(&[
                ("Ctrl+s", "save"),
                ("Ctrl+Shift+s", "save-as"),
                ("Ctrl+w", "close"),
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
                ("Alt+z", "toggle-soft-wrap"),
                ("Ctrl+d", "add-next-occurrence"),
                ("Ctrl+Shift+l", "select-all-occurrences"),
                ("Ctrl+Alt+up", "add-cursor-above"),
                ("Ctrl+Alt+down", "add-cursor-below"),
                ("Ctrl+6", "go-to-bracket"),
                ("Ctrl+{", "fold"),
                ("Ctrl+}", "unfold"),
                ("Ctrl+r", "go-to-symbol"),
                ("Ctrl+k", "hover"),
                ("f12", "go-to-definition"),
                ("Shift+f12", "find-references"),
                ("f2", "rename-symbol"),
                ("Ctrl+.", "code-actions"),
                ("Ctrl+Shift+i", "format-document"),
                ("Ctrl+ ", "trigger-completion"),
                ("Ctrl+t", "workspace-symbols"),
                ("Ctrl+f8", "next-problem"),
                ("Ctrl+Shift+f8", "previous-problem"),
                // Workbench keys: also from the file and Git panes, never
                // from terminals (shells use Ctrl+P, Ctrl+E, Alt+arrows).
                ("Ctrl+p", "quick-open"),
                ("Ctrl+Shift+f", "search-in-files"),
                ("Ctrl+e", "open-documents"),
                ("Alt+left", "go-back"),
                ("Alt+right", "go-forward"),
                ("Ctrl+Shift+b", "run-build-task"),
                ("f5", "debug-start"),
                ("Ctrl+f9", "toggle-breakpoint"),
            ])
        }
        // GNU nano's default bindings (nano 7), mapped onto Slate's actions.
        "nano" => {
            global.extend(keys(&[
                ("Ctrl+x", "quit"),
                ("Ctrl+r", "insert-file"),
                ("Ctrl+g", "help"),
                ("Ctrl+z", "suspend"),
                ("Alt+m", "toggle-mouse"),
                ("Alt+s", "toggle-soft-wrap"),
                ("Alt+n", "toggle-line-numbers"),
                ("Alt+b", "toggle-backup"),
            ]));
            keys(&[
                ("Ctrl+o", "save"),
                ("Ctrl+s", "save"),
                ("Ctrl+w", "prompt-find"),
                ("Ctrl+q", "find-previous"),
                ("Alt+w", "find-next"),
                ("Alt+q", "find-previous"),
                ("f3", "find-next"),
                ("Ctrl+\\", "prompt-replace"),
                ("Alt+r", "prompt-replace"),
                ("Ctrl+k", "cut-line"),
                ("Ctrl+u", "uncut"),
                ("Alt+6", "copy-line"),
                ("Alt+a", "mark"),
                ("Ctrl+6", "mark"),
                ("Ctrl+j", "justify"),
                ("Ctrl+t", "spell-check"),
                ("Ctrl+c", "location"),
                ("Alt+d", "word-count"),
                ("Ctrl+_", "prompt-goto"),
                ("Alt+g", "prompt-goto"),
                ("Alt+u", "undo"),
                ("Alt+e", "redo"),
                ("Ctrl+a", "line-start"),
                ("Ctrl+e", "line-end"),
                ("Ctrl+f", "move-right"),
                ("Ctrl+b", "move-left"),
                ("Ctrl+p", "move-up"),
                ("Ctrl+n", "move-down"),
                ("Ctrl+y", "page-up"),
                ("Ctrl+v", "page-down"),
                ("Alt+\\", "document-start"),
                ("Alt+/", "document-end"),
                ("Alt+}", "indent"),
                ("Alt+{", "outdent"),
                ("Ctrl+]", "indent"),
            ])
        }
        _ => bail!("keymap must be default or nano"),
    };
    Ok((global, editor, keys(TERMINAL_KEYS)))
}

impl Default for Preferences {
    fn default() -> Self {
        let (global_keys, editor_keys, terminal_keys) = keymap("default").unwrap();
        Self {
            indent_width: 4,
            insert_spaces: true,
            auto_indent: true,
            line_numbers: true,
            theme: "auto".into(),
            file_startup: StartupMode::EditorOnly,
            directory_startup: StartupMode::Workspace,
            keymap: "default".into(),
            soft_wrap: false,
            wrap_column: 80,
            hard_wrap: false,
            backup: false,
            file_recovery: false,
            tui_mouse: true,
            show_whitespace: false,
            auto_reload: true,
            minimap: true,
            font_family: String::new(),
            font_size: 11,
            terminal_scrollback: crate::terminal::DEFAULT_SCROLLBACK,
            auto_close_brackets: true,
            format_on_save: false,
            global_keys,
            editor_keys,
            terminal_keys,
        }
    }
}

/// How a setting is presented and changed in the TUI settings list.
pub enum SettingKind {
    Bool,
    Number(usize, usize),
    Choice(&'static [&'static str]),
}
pub struct Setting {
    pub name: &'static str,
    pub label: &'static str,
    pub kind: SettingKind,
}
pub const SETTINGS: &[Setting] = &[
    Setting {
        name: "file-startup",
        label: "Opening a file",
        kind: SettingKind::Choice(&["editor-only", "workspace"]),
    },
    Setting {
        name: "directory-startup",
        label: "Opening a directory",
        kind: SettingKind::Choice(&["editor-only", "workspace"]),
    },
    Setting {
        name: "indent-width",
        label: "Indent width",
        kind: SettingKind::Number(1, 16),
    },
    Setting {
        name: "insert-spaces",
        label: "Insert spaces",
        kind: SettingKind::Bool,
    },
    Setting {
        name: "auto-indent",
        label: "Auto indent",
        kind: SettingKind::Bool,
    },
    Setting {
        name: "line-numbers",
        label: "Line numbers",
        kind: SettingKind::Bool,
    },
    Setting {
        name: "theme",
        label: "Theme",
        kind: SettingKind::Choice(&["auto", "dark", "light"]),
    },
    Setting {
        name: "keymap",
        label: "Keymap",
        kind: SettingKind::Choice(&["default", "nano"]),
    },
    Setting {
        name: "soft-wrap",
        label: "Soft wrap long lines",
        kind: SettingKind::Bool,
    },
    Setting {
        name: "wrap-column",
        label: "Wrap column (justify / hard wrap)",
        kind: SettingKind::Number(10, 500),
    },
    Setting {
        name: "hard-wrap",
        label: "Hard wrap while typing",
        kind: SettingKind::Bool,
    },
    Setting {
        name: "backup",
        label: "Keep backup (NAME~) on save",
        kind: SettingKind::Bool,
    },
    Setting {
        name: "file-recovery",
        label: "Recover unsaved single files",
        kind: SettingKind::Bool,
    },
    Setting {
        name: "tui-mouse",
        label: "Terminal UI captures mouse",
        kind: SettingKind::Bool,
    },
    Setting {
        name: "show-whitespace",
        label: "Show whitespace",
        kind: SettingKind::Bool,
    },
    Setting {
        name: "auto-reload",
        label: "Reload files changed on disk",
        kind: SettingKind::Bool,
    },
    Setting {
        name: "auto-close-brackets",
        label: "Close brackets and quotes",
        kind: SettingKind::Bool,
    },
    Setting {
        name: "format-on-save",
        label: "Format on save",
        kind: SettingKind::Bool,
    },
    Setting {
        name: "terminal-scrollback",
        label: "Terminal scrollback lines",
        kind: SettingKind::Number(0, 100_000),
    },
];

impl Preferences {
    pub fn value(&self, name: &str) -> String {
        let mode = |m: StartupMode| match m {
            StartupMode::EditorOnly => "editor-only",
            StartupMode::Workspace => "workspace",
        };
        match name {
            "file-startup" => mode(self.file_startup).into(),
            "directory-startup" => mode(self.directory_startup).into(),
            "indent-width" => self.indent_width.to_string(),
            "insert-spaces" => self.insert_spaces.to_string(),
            "auto-indent" => self.auto_indent.to_string(),
            "line-numbers" => self.line_numbers.to_string(),
            "theme" => self.theme.clone(),
            "keymap" => self.keymap.clone(),
            "soft-wrap" => self.soft_wrap.to_string(),
            "wrap-column" => self.wrap_column.to_string(),
            "hard-wrap" => self.hard_wrap.to_string(),
            "backup" => self.backup.to_string(),
            "file-recovery" => self.file_recovery.to_string(),
            "tui-mouse" => self.tui_mouse.to_string(),
            "show-whitespace" => self.show_whitespace.to_string(),
            "auto-reload" => self.auto_reload.to_string(),
            "terminal-scrollback" => self.terminal_scrollback.to_string(),
            "auto-close-brackets" => self.auto_close_brackets.to_string(),
            "format-on-save" => self.format_on_save.to_string(),
            "minimap" => self.minimap.to_string(),
            "font-family" => self.font_family.clone(),
            "font-size" => self.font_size.to_string(),
            _ => String::new(),
        }
    }
    pub fn menu_items(&self) -> Vec<(&'static str, String)> {
        SETTINGS
            .iter()
            .map(|s| {
                let value = self.value(s.name);
                (
                    s.label,
                    match value.as_str() {
                        "editor-only" => "Editor only".into(),
                        "workspace" => "Full workspace".into(),
                        _ => value,
                    },
                )
            })
            .collect()
    }
    /// The value a settings-list step produces.
    pub fn stepped(&self, index: usize, backwards: bool) -> Option<(&'static str, String)> {
        let setting = SETTINGS.get(index)?;
        let current = self.value(setting.name);
        let value = match setting.kind {
            SettingKind::Bool => (current != "true").to_string(),
            SettingKind::Number(min, max) => {
                let n: usize = current.parse().unwrap_or(min);
                if backwards {
                    n.saturating_sub(1).max(min)
                } else {
                    (n + 1).min(max)
                }
                .to_string()
            }
            SettingKind::Choice(options) => {
                let i = options.iter().position(|o| *o == current).unwrap_or(0);
                let n = options.len();
                options[(i + if backwards { n - 1 } else { 1 }) % n].to_string()
            }
        };
        Some((setting.name, value))
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
        if !["default", "nano"].contains(&self.keymap.as_str()) {
            bail!("keymap must be default or nano");
        }
        if !(10..=500).contains(&self.wrap_column) {
            bail!("wrap_column must be 10–500");
        }
        if self.terminal_scrollback > 100_000 {
            bail!("terminal_scrollback must be 0–100000");
        }
        if !(6..=72).contains(&self.font_size) {
            bail!("font_size must be 6–72");
        }
        if self.font_family.len() > 200 {
            bail!("font_family is too long");
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
        let source = fs::read_to_string(path)?;
        let table: toml::Table = toml::from_str(&source).context("Invalid settings.toml")?;
        let mut settings: Self = toml::from_str(&source).context("Invalid settings.toml")?;
        // Key tables the file omits come from its chosen keymap.
        let (global, editor, terminal) = keymap(&settings.keymap)?;
        for (name, preset, field) in [
            ("global_keys", global, &mut settings.global_keys),
            ("editor_keys", editor, &mut settings.editor_keys),
            ("terminal_keys", terminal, &mut settings.terminal_keys),
        ] {
            if !table.contains_key(name) {
                *field = preset;
            }
        }
        // Settings saved by older releases contain the old complete key map.
        // Add the new defaults while preserving explicit user assignments.
        if settings.keymap == "default" {
            for (key, action) in [
                ("f10", "toggle-workspace"),
                ("Ctrl+,", "settings"),
                ("Ctrl+q", "quit"),
                ("Ctrl+o", "open"),
                ("Ctrl+Shift+o", "open-folder"),
            ] {
                settings
                    .global_keys
                    .entry(key.into())
                    .or_insert_with(|| action.into());
            }
            for (key, action) in [
                ("Ctrl+Shift+s", "save-as"),
                ("Ctrl+w", "close"),
                ("Alt+z", "toggle-soft-wrap"),
            ] {
                settings
                    .editor_keys
                    .entry(key.into())
                    .or_insert_with(|| action.into());
            }
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
        let count = SETTINGS.len();
        let prompt = self.prompt.as_mut().unwrap();
        match key.key.as_str() {
            "Escape" => self.prompt = None,
            "Up" => prompt.field = (prompt.field + count - 1) % count,
            "Down" | "Tab" => prompt.field = (prompt.field + 1) % count,
            "Left" | "Right" | "Enter" | "Space" | " " => {
                if let Some((name, value)) =
                    self.preferences.stepped(prompt.field, key.key == "Left")
                {
                    self.configure(name, &value)?;
                }
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
        self.highlights.clear();
        self.highlight_pending.clear();
    }
    pub(super) fn configure(&mut self, name: &str, value: &str) -> Result<()> {
        let mut settings = self.preferences.clone();
        let flag = |value: &str| -> Result<bool> {
            value
                .parse()
                .map_err(|_| anyhow::anyhow!("{name} must be true or false"))
        };
        match name {
            "indent-width" => settings.indent_width = value.parse()?,
            "insert-spaces" => settings.insert_spaces = flag(value)?,
            "auto-indent" => settings.auto_indent = flag(value)?,
            "line-numbers" => settings.line_numbers = flag(value)?,
            "theme" => settings.theme = value.into(),
            "file-startup" => settings.file_startup = value.parse()?,
            "directory-startup" => settings.directory_startup = value.parse()?,
            "keymap" => {
                let (global, editor, terminal) = keymap(value)?;
                settings.keymap = value.into();
                settings.global_keys = global;
                settings.editor_keys = editor;
                settings.terminal_keys = terminal;
            }
            "soft-wrap" => settings.soft_wrap = flag(value)?,
            "wrap-column" => settings.wrap_column = value.parse()?,
            "hard-wrap" => settings.hard_wrap = flag(value)?,
            "backup" => settings.backup = flag(value)?,
            "file-recovery" => settings.file_recovery = flag(value)?,
            "tui-mouse" | "mouse" => settings.tui_mouse = flag(value)?,
            "show-whitespace" => settings.show_whitespace = flag(value)?,
            "auto-reload" => settings.auto_reload = flag(value)?,
            "terminal-scrollback" => settings.terminal_scrollback = value.parse()?,
            "auto-close-brackets" => settings.auto_close_brackets = flag(value)?,
            "format-on-save" => settings.format_on_save = flag(value)?,
            "minimap" => settings.minimap = flag(value)?,
            "font-family" => settings.font_family = value.trim().into(),
            "font-size" => settings.font_size = value.parse()?,
            _ => bail!(
                "Options: {}, minimap, font-family, font-size",
                SETTINGS
                    .iter()
                    .map(|s| s.name)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keymaps_and_settings_steps() {
        let (global, editor, _) = keymap("nano").unwrap();
        assert_eq!(global["Ctrl+x"], "quit");
        assert_eq!(editor["Ctrl+k"], "cut-line");
        assert_eq!(editor["Ctrl+w"], "prompt-find");
        assert!(keymap("emacs").is_err());
        let p = Preferences::default();
        assert_eq!(p.menu_items().len(), SETTINGS.len());
        let keymap_index = SETTINGS.iter().position(|s| s.name == "keymap").unwrap();
        assert_eq!(p.stepped(keymap_index, false).unwrap().1, "nano");
        let width = SETTINGS
            .iter()
            .position(|s| s.name == "indent-width")
            .unwrap();
        assert_eq!(p.stepped(width, true).unwrap().1, "3");
        let wrap = SETTINGS.iter().position(|s| s.name == "soft-wrap").unwrap();
        assert_eq!(p.stepped(wrap, false).unwrap().1, "true");
    }
}
