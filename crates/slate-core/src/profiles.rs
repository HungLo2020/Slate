//! XDG profiles and safe editor-only overrides, resolved identically for both UIs.
use crate::{
    preferences::{Preferences, StartupMode},
    App, Command,
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        bail!("Profile names must contain 1–64 letters, numbers, '-' or '_'");
    }
    Ok(())
}
#[derive(Clone, Copy)]
pub(crate) struct EditorOptions {
    pub indent_width: usize,
    pub tab_width: usize,
    pub insert_spaces: bool,
    pub auto_indent: bool,
    pub soft_wrap: bool,
    pub wrap_column: usize,
    pub hard_wrap: bool,
    pub format_on_save: bool,
    pub auto_close_brackets: bool,
}
impl From<&Preferences> for EditorOptions {
    fn from(p: &Preferences) -> Self {
        Self {
            indent_width: p.indent_width,
            tab_width: if p.tab_width == 0 {
                p.indent_width
            } else {
                p.tab_width
            },
            insert_spaces: p.insert_spaces,
            auto_indent: p.auto_indent,
            soft_wrap: p.soft_wrap,
            wrap_column: p.wrap_column,
            hard_wrap: p.hard_wrap,
            format_on_save: p.format_on_save,
            auto_close_brackets: p.auto_close_brackets,
        }
    }
}
impl EditorOptions {
    fn install(self, p: &mut Preferences) {
        macro_rules! install { ($($field:ident),*) => { $(p.$field = self.$field;)* }; }
        install!(
            indent_width,
            tab_width,
            insert_spaces,
            auto_indent,
            soft_wrap,
            wrap_column,
            hard_wrap,
            format_on_save,
            auto_close_brackets
        );
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EditorOverrides {
    pub indent_width: Option<usize>,
    pub tab_width: Option<usize>,
    pub insert_spaces: Option<bool>,
    pub auto_indent: Option<bool>,
    pub soft_wrap: Option<bool>,
    pub wrap_column: Option<usize>,
    pub hard_wrap: Option<bool>,
    pub format_on_save: Option<bool>,
    pub auto_close_brackets: Option<bool>,
}
impl EditorOverrides {
    fn apply(&self, p: &mut EditorOptions) {
        macro_rules! apply { ($($field:ident),*) => { $(if let Some(value) = self.$field { p.$field = value; })* }; }
        apply!(
            indent_width,
            tab_width,
            insert_spaces,
            auto_indent,
            soft_wrap,
            wrap_column,
            hard_wrap,
            format_on_save,
            auto_close_brackets
        );
    }
    fn sources(&self, source: &str, out: &mut BTreeMap<String, String>) {
        macro_rules! sources { ($($field:ident),*) => { $(if self.$field.is_some() { out.insert(stringify!($field).into(), source.into()); })* }; }
        sources!(
            indent_width,
            tab_width,
            insert_spaces,
            auto_indent,
            soft_wrap,
            wrap_column,
            hard_wrap,
            format_on_save,
            auto_close_brackets
        );
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Overrides {
    editor: EditorOverrides,
    language: BTreeMap<String, EditorOverrides>,
}
impl Overrides {
    fn read(path: &Path) -> Result<Self> {
        let result: Self = crate::config::read(path)
            .map_err(anyhow::Error::msg)?
            .unwrap_or_default();
        for overrides in std::iter::once(&result.editor).chain(result.language.values()) {
            let mut p = Preferences::default();
            let mut options = EditorOptions::from(&p);
            overrides.apply(&mut options);
            options.install(&mut p);
            p.validate()?;
        }
        Ok(result)
    }
}
#[derive(Serialize, Deserialize)]
struct Profile {
    preferences: Preferences,
    layout: crate::layout::Node,
    editor_only: bool,
}
fn path(name: &str) -> PathBuf {
    crate::paths::config_dir()
        .join("profiles")
        .join(format!("{name}.toml"))
}
fn read(name: &str) -> Result<Profile> {
    validate_name(name)?;
    let profile: Profile = crate::config::read(&path(name))
        .map_err(anyhow::Error::msg)?
        .context("Profile does not exist")?;
    profile.preferences.validate()?;
    if !profile.layout.validate() {
        bail!("Invalid profile layout");
    }
    Ok(profile)
}

pub(crate) struct Layers {
    pub base: Preferences,
    pub(crate) editorconfig: crate::editorconfig::Cache,
    pub names: Vec<String>,
    global: Overrides,
    workspace: Overrides,
    languages: Vec<crate::languages::Language>,
    active_path: Option<Option<PathBuf>>,
    pub startup: Option<bool>,
    pub layout: Option<crate::layout::Node>,
}
impl Default for Layers {
    fn default() -> Self {
        Self {
            base: Preferences::default(),
            editorconfig: Default::default(),
            names: vec!["default".into(), "minimal".into(), "workspace".into()],
            global: Overrides::default(),
            workspace: Overrides::default(),
            languages: Vec::new(),
            active_path: None,
            startup: None,
            layout: None,
        }
    }
}
impl Layers {
    pub(crate) fn load(root: &Path) -> Result<Self> {
        Self::load_with_preferences(root, Preferences::load()?)
    }
    fn load_with_preferences(root: &Path, global_preferences: Preferences) -> Result<Self> {
        let profile =
            if ["default", "minimal", "workspace"].contains(&global_preferences.profile.as_str()) {
                None
            } else {
                Some(read(&global_preferences.profile)?)
            };
        let mut base = profile
            .as_ref()
            .map(|p| p.preferences.clone())
            .unwrap_or_else(|| global_preferences.clone());
        base.profile = global_preferences.profile;
        if base.profile == "minimal" {
            base.file_startup = StartupMode::EditorOnly;
            base.directory_startup = StartupMode::EditorOnly;
        }
        if base.profile == "workspace" {
            base.file_startup = StartupMode::Workspace;
            base.directory_startup = StartupMode::Workspace;
        }
        let mut names = vec!["default".into(), "minimal".into(), "workspace".into()];
        if let Ok(entries) = std::fs::read_dir(crate::paths::config_dir().join("profiles")) {
            for entry in entries.flatten() {
                if entry.path().extension().is_some_and(|e| e == "toml") {
                    if let Some(name) = entry.path().file_stem().and_then(|s| s.to_str()) {
                        if validate_name(name).is_ok() && !names.iter().any(|n| n == name) {
                            names.push(name.into());
                        }
                    }
                }
            }
        }
        let mut errors = Vec::new();
        let languages = crate::languages::all(&mut errors);
        if let Some(error) = errors.first() {
            bail!("{error}");
        }
        Ok(Self {
            editorconfig: Default::default(),
            base,
            names,
            active_path: None,
            startup: profile.as_ref().map(|p| p.editor_only),
            layout: profile.map(|p| p.layout),
            global: Overrides::read(&crate::paths::config_dir().join("editor-overrides.toml"))?,
            workspace: Overrides::read(&root.join(".slate/settings.toml"))?,
            languages,
        })
    }
    fn language(&self, path: Option<&Path>) -> Option<String> {
        let path = path?;
        crate::languages::find(
            &self.languages,
            path,
            &crate::highlight::syntax_for(Some(path), "").name,
        )
        .map(|l| l.id())
        .or_else(|| path.extension().map(|s| s.to_string_lossy().into_owned()))
    }
    fn resolve(&self, path: Option<&Path>) -> EditorOptions {
        let mut p = EditorOptions::from(&self.base);
        self.global.editor.apply(&mut p);
        if self.base.tab_width == 0 && self.global.editor.tab_width.is_none() {
            p.tab_width = p.indent_width;
        }
        if let Some(path) = path {
            if let Ok(settings) = self.editorconfig.get(path) {
                settings.apply(&mut p);
            }
        }
        self.workspace.editor.apply(&mut p);
        if let Some(language) = self.language(path) {
            if let Some(overrides) = self.global.language.get(&language) {
                overrides.apply(&mut p);
            }
            if let Some(overrides) = self.workspace.language.get(&language) {
                overrides.apply(&mut p);
            }
        }
        // An override layer may also set zero: it follows the indent width,
        // and tab stops are never zero columns wide.
        if p.tab_width == 0 {
            p.tab_width = p.indent_width;
        }
        p
    }
}
impl App {
    pub(crate) fn preferences_for_document(&self, id: u64) -> EditorOptions {
        self.preference_layers
            .resolve(self.documents.get(&id).and_then(|d| d.path.as_deref()))
    }
    pub(crate) fn sync_document_preferences(&mut self) {
        let path = self
            .active_editor()
            .and_then(|id| self.documents[&self.views[&id].document].path.clone());
        if self.preference_layers.active_path.as_ref() == Some(&path) {
            return;
        }
        let options = self.preference_layers.resolve(path.as_deref());
        self.preferences = self.preference_layers.base.clone();
        options.install(&mut self.preferences);
        self.preference_layers.active_path = Some(path);
    }
    pub(crate) fn preference_sources(&self) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        self.preference_layers
            .global
            .editor
            .sources("Global editor overrides", &mut out);
        if let Some(path) = self
            .active_editor()
            .and_then(|id| self.documents[&self.views[&id].document].path.as_deref())
        {
            if let Ok(settings) = self.preference_layers.editorconfig.get(path) {
                if settings.indent.is_some() {
                    out.insert("indent_width".into(), "EditorConfig".into());
                }
                if settings.tab.is_some() {
                    out.insert("tab_width".into(), "EditorConfig".into());
                }
                if settings.spaces.is_some() {
                    out.insert("insert_spaces".into(), "EditorConfig".into());
                }
            }
        }
        self.preference_layers
            .workspace
            .editor
            .sources("Workspace settings", &mut out);
        let path = self
            .active_editor()
            .and_then(|id| self.documents[&self.views[&id].document].path.as_deref());
        if let Some(language) = self.preference_layers.language(path) {
            if let Some(p) = self.preference_layers.global.language.get(&language) {
                p.sources(&format!("Global {language} settings"), &mut out);
            }
            if let Some(p) = self.preference_layers.workspace.language.get(&language) {
                p.sources(&format!("Workspace {language} settings"), &mut out);
            }
        }
        out
    }
    /// Change the active profile's saved preferences: settings.toml, or a
    /// custom profile's `[preferences]` (see `edit_preferences`).
    pub(crate) fn edit_profile_preferences(
        &self,
        change: impl FnOnce(&mut Preferences) -> Result<()>,
    ) -> Result<()> {
        match self.preference_layers.base.profile.as_str() {
            "default" | "minimal" | "workspace" => crate::preferences::edit_preferences(
                &Preferences::path(),
                None,
                Preferences::parse,
                change,
            ),
            name => {
                validate_name(name)?;
                crate::preferences::edit_preferences(
                    &path(name),
                    Some("preferences"),
                    |source| Ok(toml::from_str::<Profile>(source)?.preferences),
                    change,
                )
            }
        }
    }
    pub(crate) fn select_profile(&mut self, name: &str) -> Result<()> {
        validate_name(name)?;
        let profile = if ["default", "minimal", "workspace"].contains(&name) {
            None
        } else {
            Some(read(name)?)
        };
        let mut global = Preferences::load()?;
        global.profile = name.into();
        // Validate the profile and both override layers before persisting selection.
        let layers = Layers::load_with_preferences(&self.root, global)?;
        crate::preferences::edit_preferences(
            &Preferences::path(),
            None,
            Preferences::parse,
            |p| {
                p.profile = name.into();
                Ok(())
            },
        )?;
        self.install_preferences(layers);
        if let Some(profile) = profile {
            self.restore_layout(profile.layout)?;
            self.editor_only = profile.editor_only;
        } else if name == "minimal" {
            self.execute(Command::EditorOnly)?;
        } else if name == "workspace" {
            self.execute(Command::ShowWorkspace)?;
        }
        self.status = format!("Loaded profile {name}");
        Ok(())
    }
    pub(crate) fn profile_action(&mut self, action: &str, argument: &str) -> Result<()> {
        if argument.trim().is_empty() {
            return self.execute(Command::Prompt {
                kind: action.into(),
            });
        }
        validate_name(argument)?;
        if action == "profile-load" {
            return self.select_profile(argument);
        }
        if ["default", "minimal", "workspace"].contains(&argument) {
            bail!("Choose a custom profile name");
        }
        // Saved settings, without this session's launch options.
        let preferences = if self.ignore_rc {
            Preferences::default()
        } else {
            Layers::load(&self.root)?.base
        };
        let profile = Profile {
            preferences,
            layout: self.layout.clone(),
            editor_only: self.editor_only,
        };
        // Reject malformed existing files instead of silently overwriting them.
        if path(argument).exists() {
            read(argument)?;
        }
        crate::fsio::write_private(
            &path(argument),
            toml::to_string_pretty(&profile)?.as_bytes(),
        )?;
        self.load_preferences();
        self.status = format!("Saved profile {argument}");
        Ok(())
    }
}
