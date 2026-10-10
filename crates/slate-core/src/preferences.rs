use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::PathBuf};

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
    pub profile: String,
    pub indent_width: usize,
    /// Zero follows indent_width.
    pub tab_width: usize,
    pub history_log: bool,
    pub locking: bool,
    pub insert_spaces: bool,
    pub auto_indent: bool,
    pub line_numbers: bool,
    pub theme: String,
    /// Content themes are independent of desktop menus and dialogs.
    pub editor_theme: String,
    pub terminal_theme: String,
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
    /// Let programs in terminals set the clipboard (OSC 52).
    pub terminal_clipboard: bool,
    /// Open completions automatically while typing.
    pub complete_while_typing: bool,
    /// Enter accepts the selected completion (Tab always does).
    pub accept_completion_on_enter: bool,
    pub global_keys: BTreeMap<String, String>,
    pub editor_keys: BTreeMap<String, String>,
    pub terminal_keys: BTreeMap<String, String>,
    /// Keys that act only while debugging, ahead of the other tables.
    pub debug_keys: BTreeMap<String, String>,
    /// The key tables' revision; older files gain new default bindings.
    /// Files written before revisions existed have none (0).
    #[serde(default)]
    pub keys_revision: u32,
}

/// Debugger keys: VS Code's function keys, and Alt+Shift letters for
/// terminals that keep F10/F11 for themselves.
pub const DEBUG_KEYS: &[(&str, &str)] = &[
    ("f9", "toggle-breakpoint"),
    ("f10", "debug-step-over"),
    ("f11", "debug-step-into"),
    ("Shift+f11", "debug-step-out"),
    ("Shift+f5", "debug-stop"),
    ("Alt+Shift+n", "debug-step-over"),
    ("Alt+Shift+i", "debug-step-into"),
    ("Alt+Shift+o", "debug-step-out"),
    ("Alt+Shift+c", "debug-continue"),
];

/// Bumped whenever a keymap gains default bindings.
pub const KEYS_REVISION: u32 = 3;

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
                ("Alt+h", "hover"),
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
                // Desktop editor basics.
                ("Ctrl+n", "new"),
                ("Ctrl+tab", "next-tab"),
                ("Ctrl+pagedown", "next-tab"),
                ("Ctrl+Shift+tab", "previous-tab"),
                ("Ctrl+pageup", "previous-tab"),
                ("Shift+f1", "help"),
                // Alternatives for chords most terminals cannot send
                // (Ctrl+Shift+letter, Ctrl+., Ctrl+Shift+[).
                ("Alt+Shift+f", "format-document"),
                ("Alt+enter", "code-actions"),
                ("Alt+Shift+l", "select-all-occurrences"),
                ("Alt+Shift+b", "run-build-task"),
                ("Alt+Shift+s", "search-in-files"),
                ("Alt+-", "fold"),
                ("Alt+=", "unfold"),
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
                // IDE features on keys nano leaves free.
                ("f12", "go-to-definition"),
                ("Shift+f12", "find-references"),
                ("f5", "debug-start"),
                ("Ctrl+f9", "toggle-breakpoint"),
                ("Shift+f1", "help"),
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
            profile: "default".into(),
            indent_width: 4,
            tab_width: 0,
            history_log: false,
            locking: false,
            insert_spaces: true,
            auto_indent: true,
            line_numbers: true,
            theme: "auto".into(),
            editor_theme: "dark".into(),
            terminal_theme: "dark".into(),
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
            terminal_clipboard: false,
            complete_while_typing: true,
            accept_completion_on_enter: true,
            global_keys,
            editor_keys,
            terminal_keys,
            debug_keys: keys(DEBUG_KEYS),
            keys_revision: KEYS_REVISION,
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
        name: "profile",
        label: "Profile",
        kind: SettingKind::Choice(&["default", "minimal", "workspace"]),
    },
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
        name: "editor-theme",
        label: "Editor theme",
        kind: SettingKind::Choice(&["auto", "dark", "light"]),
    },
    Setting {
        name: "terminal-theme",
        label: "Terminal theme",
        kind: SettingKind::Choice(&["auto", "dark", "light", "editor"]),
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
        name: "complete-while-typing",
        label: "Complete while typing",
        kind: SettingKind::Bool,
    },
    Setting {
        name: "accept-completion-on-enter",
        label: "Enter accepts a completion",
        kind: SettingKind::Bool,
    },
    Setting {
        name: "terminal-clipboard",
        label: "Terminal programs may set the clipboard",
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
            "profile" => self.profile.clone(),
            "file-startup" => mode(self.file_startup).into(),
            "directory-startup" => mode(self.directory_startup).into(),
            "indent-width" => self.indent_width.to_string(),
            "insert-spaces" => self.insert_spaces.to_string(),
            "auto-indent" => self.auto_indent.to_string(),
            "line-numbers" => self.line_numbers.to_string(),
            "theme" | "editor-theme" => self.editor_theme.clone(),
            "terminal-theme" => self.terminal_theme.clone(),
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
            "terminal-clipboard" => self.terminal_clipboard.to_string(),
            "complete-while-typing" => self.complete_while_typing.to_string(),
            "accept-completion-on-enter" => self.accept_completion_on_enter.to_string(),
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
        crate::profiles::validate_name(&self.profile)?;
        if !(1..=16).contains(&self.indent_width) {
            bail!("indent_width must be 1–16");
        }
        if !["auto", "dark", "light"].contains(&self.theme.as_str()) {
            bail!("theme must be auto, dark or light");
        }
        if !["auto", "dark", "light"].contains(&self.editor_theme.as_str()) {
            bail!("editor_theme must be auto, dark or light");
        }
        if !["auto", "dark", "light", "editor"].contains(&self.terminal_theme.as_str()) {
            bail!("terminal_theme must be auto, dark, light or editor");
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
        if self.tab_width > 32 {
            bail!("tab_width must be 0–32");
        }
        if self.global_keys.len()
            + self.editor_keys.len()
            + self.terminal_keys.len()
            + self.debug_keys.len()
            > 320
        {
            bail!("Too many key bindings");
        }
        Ok(())
    }
    pub fn load() -> Result<Self> {
        let path = Self::path();
        if !path.exists() {
            return Ok(Self::default());
        }
        Self::parse(&fs::read_to_string(path)?)
    }
    /// Settings as a settings.toml text defines them, older files migrated.
    pub(crate) fn parse(source: &str) -> Result<Self> {
        let table: toml::Table = toml::from_str(source).context("Invalid settings.toml")?;
        let mut settings: Self = toml::from_str(source).context("Invalid settings.toml")?;
        // Preserve an older explicit light/dark choice. Older automatic
        // settings gain the darker content defaults without changing Qt chrome.
        if !table.contains_key("editor_theme") && settings.theme != "auto" {
            settings.editor_theme = settings.theme.clone();
        }
        // Key tables the file omits come from its chosen keymap.
        let (global, editor, terminal) = keymap(&settings.keymap)?;
        for (name, preset, field) in [
            ("global_keys", &global, &mut settings.global_keys),
            ("editor_keys", &editor, &mut settings.editor_keys),
            ("terminal_keys", &terminal, &mut settings.terminal_keys),
        ] {
            if !table.contains_key(name) {
                *field = preset.clone();
            }
        }
        if !table.contains_key("debug_keys") {
            settings.debug_keys = keys(DEBUG_KEYS);
        }
        // Settings saved by older releases contain that release's complete
        // key tables. Add the keymap's newer defaults on keys the file does
        // not assign, keeping every explicit assignment.
        if settings.keys_revision < KEYS_REVISION {
            for (field, preset) in [
                (&mut settings.global_keys, &global),
                (&mut settings.editor_keys, &editor),
                (&mut settings.terminal_keys, &terminal),
            ] {
                for (key, action) in preset {
                    field.entry(key.clone()).or_insert_with(|| action.clone());
                }
            }
            settings.keys_revision = KEYS_REVISION;
        }
        settings.validate()?;
        Ok(settings)
    }
    pub fn save(&self) -> Result<()> {
        self.validate()?;
        crate::fsio::write_private(&Self::path(), toml::to_string_pretty(self)?.as_bytes())
    }
}

/// Change one preferences file in place. The file is re-read under a lock
/// (another instance or the user may have edited it), `change` applies to
/// what it says (`read` parses it), and only settings whose values change
/// are written: comments, layout and everything else stay as they are, and
/// session-only state (launch options, profile defaults) never reaches it.
/// `section` names the table holding the preferences, or the file's root.
pub(crate) fn edit_preferences(
    path: &std::path::Path,
    section: Option<&str>,
    read: impl Fn(&str) -> Result<Preferences>,
    change: impl FnOnce(&mut Preferences) -> Result<()>,
) -> Result<()> {
    crate::fsio::with_lock(path, || {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let source = match fs::read_to_string(path) {
            Ok(source) => source,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e.into()),
        };
        let before = read(&source)
            .and_then(|p| p.validate().map(|()| p))
            .map_err(|e| anyhow::anyhow!("Not saving over {name} until it is fixed: {e:#}"))?;
        let mut after = before.clone();
        change(&mut after)?;
        after.validate()?;
        let old = toml::Table::try_from(&before)?;
        let new = toml::Table::try_from(&after)?;
        let mut keys: Vec<&String> = new.keys().filter(|k| old.get(*k) != new.get(*k)).collect();
        // Key tables are migrated together (see `load`): writing one writes
        // them all with the revision they belong to.
        if keys.iter().any(|k| k.ends_with("_keys")) {
            keys = new
                .keys()
                .filter(|k| k.ends_with("_keys") || *k == "keys_revision" || keys.contains(k))
                .collect();
        }
        let mut document: toml_edit::DocumentMut =
            source.parse().with_context(|| format!("Invalid {name}"))?;
        let table = match section {
            None => document.as_table_mut(),
            Some(section) => document
                .get_mut(section)
                .and_then(toml_edit::Item::as_table_mut)
                .with_context(|| format!("{name} has no [{section}] table"))?,
        };
        for key in keys {
            let fragment =
                toml::to_string(&toml::Table::from_iter([(key.clone(), new[key].clone())]))?;
            let mut fragment: toml_edit::DocumentMut = fragment.parse()?;
            let item = fragment.remove(key).context("Setting did not serialize")?;
            merge(table, key, item);
        }
        let text = document.to_string();
        anyhow::ensure!(
            toml::Table::try_from(read(&text)?)? == new,
            "Could not update {name} without changing other settings"
        );
        crate::fsio::write_private(path, text.as_bytes())
    })
}

/// Set `key` in `table`, keeping what surrounds unchanged values: comments
/// beside a value and inside a table (such as a key table) survive.
fn merge(table: &mut toml_edit::Table, key: &str, item: toml_edit::Item) {
    use toml_edit::Item;
    match (table.get_mut(key), item) {
        (Some(Item::Value(old)), Item::Value(mut value)) => {
            if old.to_string().trim() != value.to_string().trim() {
                *value.decor_mut() = old.decor().clone();
                *old = value;
            }
        }
        (Some(Item::Table(old)), Item::Table(new)) => {
            let stale: Vec<String> = old
                .iter()
                .map(|(k, _)| k.to_string())
                .filter(|k| !new.contains_key(k))
                .collect();
            for k in stale {
                old.remove(&k);
            }
            for (k, value) in new {
                merge(old, &k, value);
            }
        }
        (_, item) => {
            table.insert(key, item);
        }
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
                if SETTINGS[prompt.field].name == "profile" {
                    let names = &self.preference_layers.names;
                    let index = names
                        .iter()
                        .position(|n| n == &self.preferences.profile)
                        .unwrap_or(0);
                    let next = if key.key == "Left" {
                        (index + names.len() - 1) % names.len()
                    } else {
                        (index + 1) % names.len()
                    };
                    let name = names[next].clone();
                    self.select_profile(&name)?;
                    return Ok(());
                }
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
        match if self.ignore_rc {
            Ok(crate::profiles::Layers::default())
        } else {
            crate::profiles::Layers::load(&self.root)
        } {
            Ok(layers) => self.install_preferences(layers),
            Err(e) => self.status = format!("Settings error: {e:#}"),
        }
    }
    pub(crate) fn install_preferences(&mut self, mut layers: crate::profiles::Layers) {
        crate::cli::apply_launch_options(&mut layers.base, &self.launch_options);
        self.preference_layers = layers;
        self.sync_document_preferences();
        self.overview_cache.clear();
        self.overview_pending.clear();
        self.overview_revision += 1;
        self.apply_pane_colors();
        self.highlights.clear();
        self.highlight_pending.clear();
    }
    pub(super) fn configure(&mut self, name: &str, value: &str) -> Result<()> {
        // Saving writes the whole file; never replace one the user can still fix.
        if Preferences::path().exists() {
            if let Err(e) = Preferences::load() {
                bail!("Not saving over settings.toml until it is fixed: {e:#}");
            }
        }
        if name == "profile" {
            return self.select_profile(value);
        }
        let flag = |value: &str| -> Result<bool> {
            value
                .parse()
                .map_err(|_| anyhow::anyhow!("{name} must be true or false"))
        };
        // Applied to the settings file as it is now, not to this session's
        // settings (which include launch options and profile defaults).
        self.edit_profile_preferences(|settings| {
            match name {
                "indent-width" => settings.indent_width = value.parse()?,
                "tab-width" => settings.tab_width = value.parse()?,
                "history-log" => settings.history_log = flag(value)?,
                "locking" => settings.locking = flag(value)?,
                "insert-spaces" => settings.insert_spaces = flag(value)?,
                "auto-indent" => settings.auto_indent = flag(value)?,
                "line-numbers" => settings.line_numbers = flag(value)?,
                "theme" | "editor-theme" => {
                    settings.theme = value.into();
                    settings.editor_theme = value.into();
                }
                "terminal-theme" => settings.terminal_theme = value.into(),
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
                "terminal-clipboard" => settings.terminal_clipboard = flag(value)?,
                "complete-while-typing" => settings.complete_while_typing = flag(value)?,
                "accept-completion-on-enter" => settings.accept_completion_on_enter = flag(value)?,
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
            Ok(())
        })?;
        self.load_preferences();
        self.status = format!("Set {name} to {value}");
        Ok(())
    }
    pub(super) fn apply_pane_colors(&mut self) {
        let automatic = || match self.system_colors.as_ref() {
            Some((fg, bg, selection, accent, selection_fg)) => {
                crate::theme::Palette::new(fg, bg, selection, selection_fg, accent)
            }
            None => crate::theme::Palette::dark(false),
        };
        let editor = match self.preferences.editor_theme.as_str() {
            "auto" => automatic(),
            "light" => crate::theme::Palette::light(),
            _ => crate::theme::Palette::dark(false),
        };
        self.terminal_colors = match self.preferences.terminal_theme.as_str() {
            "auto" => automatic(),
            "light" => crate::theme::Palette::light(),
            "editor" => editor.clone(),
            _ => crate::theme::Palette::dark(true),
        };
        self.colors = (
            editor.foreground,
            editor.background,
            editor.selection,
            editor.accent,
        );
        self.selection_foreground = editor.selection_foreground;
    }
    pub(super) fn light_theme(&self) -> bool {
        crate::theme::is_light(&self.colors.1)
    }
}

impl App {
    pub(crate) fn set_shortcut(&mut self, scope: &str, chord: &str, command: &str) -> Result<()> {
        if crate::preferences::Preferences::path().exists() {
            crate::preferences::Preferences::load()
                .context("Fix settings.toml before changing shortcuts")?;
        }
        anyhow::ensure!(chord.len() <= 64 && !chord.is_empty(), "Enter a key chord");
        let mut parts: Vec<_> = chord.split('+').collect();
        let key = parts.pop().unwrap();
        anyhow::ensure!(
            !key.is_empty() && parts.iter().all(|p| ["Ctrl", "Alt", "Shift"].contains(p)),
            "Use Ctrl+Shift+s or a key name such as F9"
        );
        anyhow::ensure!(
            command.is_empty() || self.resolve_command_line(command).is_some(),
            "Unknown command"
        );
        self.edit_profile_preferences(|p| {
            let table = match scope {
                "global" => &mut p.global_keys,
                "editor" => &mut p.editor_keys,
                "terminal" => &mut p.terminal_keys,
                "debug" => &mut p.debug_keys,
                _ => bail!("Unknown shortcut scope"),
            };
            if command.is_empty() {
                table.remove(chord);
            } else {
                table.insert(chord.into(), command.into());
            }
            Ok(())
        })?;
        self.load_preferences();
        self.status = "Shortcut saved".into();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_themes_are_independent_and_survive_reload() {
        let _env = crate::paths::TEST_ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
        std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
        let file = dir.path().join("file.txt");
        std::fs::write(&file, "hello").unwrap();
        let mut app = crate::App::new(&file).unwrap();
        let default = app.snapshot(100, 30, 0, 1, 1, 0);
        assert_eq!(default.background, "#1b1e26");
        assert_eq!(default.terminal_background, "#14171c");
        app.dispatch(crate::Command::Theme {
            foreground: "#101010".into(),
            background: "#ffffff".into(),
            selection: "#abcdef".into(),
            accent: "#123456".into(),
            selection_foreground: "#010101".into(),
        });
        assert_eq!(
            app.colors.1, "#1b1e26",
            "Native desktop changes must not replace explicit pane themes"
        );
        app.configure("terminal-theme", "auto").unwrap();
        assert_eq!(app.terminal_colors.background, "#ffffff");
        assert_eq!(app.colors.1, "#1b1e26");
        app.configure("editor-theme", "light").unwrap();
        assert_eq!(app.preferences.value("theme"), "light");
        app.configure("terminal-theme", "dark").unwrap();
        app.load_preferences();
        assert!(app.light_theme());
        assert!(!app.terminal_colors.is_light());
        app.configure("terminal-theme", "editor").unwrap();
        assert_eq!(app.terminal_colors.background, app.colors.1);
        assert_eq!(app.terminal_colors.selection, app.colors.2);
        app.configure("editor-theme", "auto").unwrap();
        assert_eq!(app.colors.1, "#ffffff");
        assert_eq!(app.terminal_colors.selection, "#abcdef");
        assert_eq!(Preferences::load().unwrap().terminal_theme, "editor");
        std::fs::write(Preferences::path(), "theme = \"light\"\n").unwrap();
        let migrated = Preferences::load().unwrap();
        assert_eq!(migrated.editor_theme, "light");
        assert_eq!(migrated.terminal_theme, "dark");
    }

    #[test]
    fn saving_a_setting_edits_the_file_without_session_state() {
        let _env = crate::paths::TEST_ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path().join("config"));
        std::env::set_var("XDG_STATE_HOME", dir.path().join("state"));
        let file = dir.path().join("file.txt");
        std::fs::write(&file, "hello").unwrap();
        std::fs::create_dir_all(dir.path().join("config/slate")).unwrap();
        let original = "# My settings\nprofile = \"minimal\"\nindent_width = 3 # narrow\n\n[editor_keys]\n# mine\n\"Ctrl+p\" = \"print\"\n";
        std::fs::write(Preferences::path(), original).unwrap();
        // Launch options (-S soft wrap, -B backups) are for this session only.
        let args = ["-S", "-B", file.to_str().unwrap()];
        let launch = crate::cli::parse(args.iter().map(std::ffi::OsString::from)).unwrap();
        let mut app = crate::App::launch(&launch, None).unwrap();
        assert!(app.preferences.soft_wrap && app.preferences.backup);
        assert_eq!(app.preferences.directory_startup, StartupMode::EditorOnly);
        // Another instance (or the user) changes the file meanwhile.
        let edited = original.replace("indent_width = 3", "indent_width = 2");
        std::fs::write(Preferences::path(), &edited).unwrap();
        app.configure("line-numbers", "false").unwrap();
        let saved = std::fs::read_to_string(Preferences::path()).unwrap();
        assert!(
            saved.starts_with("# My settings\nprofile = \"minimal\"\nindent_width = 2 # narrow\n"),
            "{saved}"
        );
        assert!(saved.contains("line_numbers = false"), "{saved}");
        for session in ["soft_wrap", "backup", "file_startup", "directory_startup"] {
            assert!(!saved.contains(session), "{session} leaked into {saved}");
        }
        assert_eq!(app.preferences.indent_width, 2, "Reloaded from disk");
        assert!(app.preferences.soft_wrap, "Launch options still apply");
        // A shortcut keeps the table's comment and the user's binding.
        app.set_shortcut("editor", "Ctrl+Shift+k", "cut-line")
            .unwrap();
        let saved = std::fs::read_to_string(Preferences::path()).unwrap();
        assert!(saved.contains("# mine\n"), "{saved}");
        let loaded = Preferences::load().unwrap();
        assert_eq!(loaded.editor_keys["Ctrl+p"], "print");
        assert_eq!(loaded.editor_keys["Ctrl+Shift+k"], "cut-line");
        assert!(!loaded.soft_wrap && !loaded.backup && !loaded.line_numbers);
        assert_eq!(loaded.directory_startup, StartupMode::Workspace);
        // A file that does not parse is never replaced.
        std::fs::write(Preferences::path(), "indent_width = [\n").unwrap();
        assert!(app.configure("line-numbers", "true").is_err());
        assert!(app.set_shortcut("editor", "Ctrl+Shift+k", "").is_err());
        assert_eq!(
            std::fs::read_to_string(Preferences::path()).unwrap(),
            "indent_width = [\n"
        );
    }

    #[test]
    fn older_settings_gain_new_default_keys_without_losing_their_own() {
        let _env = crate::paths::TEST_ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path());
        std::fs::create_dir_all(dir.path().join("slate")).unwrap();
        // Saved by an older release: complete tables, no revision, one change.
        std::fs::write(
            dir.path().join("slate/settings.toml"),
            "keymap = \"default\"\n[global_keys]\n\"Ctrl+q\" = \"quit\"\n[editor_keys]\n\"Ctrl+s\" = \"save\"\n\"Ctrl+p\" = \"print\"\n[terminal_keys]\n",
        )
        .unwrap();
        let loaded = Preferences::load().unwrap();
        assert_eq!(
            loaded.editor_keys["Ctrl+p"], "print",
            "the user's own binding stays"
        );
        assert_eq!(loaded.editor_keys["f12"], "go-to-definition");
        assert_eq!(loaded.editor_keys["Ctrl+d"], "add-next-occurrence");
        assert_eq!(loaded.debug_keys["f10"], "debug-step-over");
        assert_eq!(loaded.keys_revision, KEYS_REVISION);
    }

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
