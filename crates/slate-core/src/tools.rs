//! External tools: commands that extend the editor without a plugin API.
//! A tool reads the selection or document on standard input and its output
//! replaces the selection, is inserted, opens as a document, or is shown in
//! the status bar. Tools appear in the command palette as "Tool: NAME" and
//! may have a key.
//!
//! `tools.toml` in the config directory holds personal tools; the
//! workspace's `.slate/tools.toml` holds project tools, which run only in a
//! trusted workspace.
//!
//! ```toml
//! [[tool]]
//! name = "Sort lines"
//! command = "sort"
//! input = "selection"    # selection (or the whole document), document, none
//! output = "replace"     # replace, insert, document, status, none
//! key = "Alt+s"
//! ```
//!
//! The command runs with `sh -c` in the document's folder, with
//! `SLATE_FILE`, `SLATE_LINE`, `SLATE_COLUMN`, `SLATE_SELECTION` and
//! `SLATE_ROOT` set.
use crate::{services::Reply, App};
use anyhow::{bail, Result};
use serde::Deserialize;
use std::{path::Path, time::Duration};

const TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Tool {
    pub name: String,
    pub command: String,
    #[serde(default = "selection")]
    pub input: String,
    #[serde(default = "replace")]
    pub output: String,
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub description: String,
    /// Defined by the workspace rather than the user.
    #[serde(skip)]
    pub project: bool,
    /// The command-palette identity: `tool:` and the slugged name.
    #[serde(skip)]
    pub id: String,
}
fn selection() -> String {
    "selection".into()
}
fn replace() -> String {
    "replace".into()
}

#[derive(Deserialize)]
struct File {
    #[serde(default)]
    tool: Vec<Tool>,
}

fn slug(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn read(path: &Path, project: bool) -> Vec<Tool> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| toml::from_str::<File>(&s).ok())
        .map(|f| f.tool)
        .unwrap_or_default()
        .into_iter()
        .filter(|t| {
            ["selection", "document", "none"].contains(&t.input.as_str())
                && ["replace", "insert", "document", "status", "none"].contains(&t.output.as_str())
        })
        .map(|mut t| {
            t.project = project;
            t.id = format!("tool:{}", slug(&t.name));
            t
        })
        .collect()
}

/// Personal tools, then project tools (a project tool never replaces a
/// personal one of the same name).
pub fn load(root: &Path) -> Vec<Tool> {
    let mut tools = read(&crate::paths::config_dir().join("tools.toml"), false);
    for tool in read(&root.join(".slate/tools.toml"), true) {
        if !tools.iter().any(|t| t.id == tool.id) {
            tools.push(tool);
        }
    }
    tools
}

impl App {
    pub(crate) fn reload_tools(&mut self) {
        self.tools = load(&self.root);
    }

    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }

    /// Run a tool by its slug (the part after `tool:`) or name.
    pub(crate) fn run_tool(&mut self, which: &str) -> Result<()> {
        let tool = self
            .tools
            .iter()
            .find(|t| t.id == format!("tool:{which}") || t.name == which)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Unknown tool: {which}"))?;
        if tool.project && !self.trusted {
            self.ask_trust(
                "Project tools run commands from this folder",
                crate::Command::Action {
                    name: "run-tool".into(),
                    argument: which.to_string(),
                },
            );
            return Ok(());
        }
        let editor = self.active_editor();
        if editor.is_none()
            && (tool.input != "none" || matches!(tool.output.as_str(), "replace" | "insert"))
        {
            bail!("Focus an editor to run {}", tool.name);
        }
        let mut env: Vec<(String, String)> = vec![(
            "SLATE_ROOT".into(),
            self.root.to_string_lossy().into_owned(),
        )];
        let mut input = String::new();
        let mut target = None;
        let mut cwd = self.root.clone();
        if let Some(id) = editor {
            let v = &self.views[&id];
            let d = &self.documents[&v.document];
            let (a, b) = {
                let anchor = v.anchor.unwrap_or(v.cursor);
                (v.cursor.min(anchor), v.cursor.max(anchor))
            };
            let (line, column) = d.line_col(v.cursor, self.preferences.indent_width);
            if let Some(path) = &d.path {
                env.push(("SLATE_FILE".into(), path.to_string_lossy().into_owned()));
                if let Some(parent) = path.parent() {
                    cwd = parent.to_path_buf();
                }
            }
            env.push(("SLATE_LINE".into(), (line + 1).to_string()));
            env.push(("SLATE_COLUMN".into(), (column + 1).to_string()));
            env.push(("SLATE_SELECTION".into(), d.slice(a, b).into_owned()));
            // A selection tool without a selection works on the document.
            let range = match tool.input.as_str() {
                "selection" if a < b => (a, b),
                "selection" | "document" => (0, d.len()),
                _ => (a, b),
            };
            if tool.input != "none" {
                input = d.slice(range.0, range.1).into_owned();
            }
            let range = if tool.output == "insert" {
                (b, b)
            } else {
                range
            };
            target = Some((v.document, d.content_version(), range));
        }
        let command = vec!["sh".to_string(), "-c".into(), tool.command.clone()];
        let name = tool.name.clone();
        let output = tool.output.clone();
        self.services.background(move || {
            let result = crate::format::pipe_env(&command, &cwd, &input, &env, TIMEOUT);
            Reply::ToolDone(name, output, target, result)
        });
        self.status = format!("Running {}…", tool.name);
        Ok(())
    }

    pub(crate) fn tool_done(
        &mut self,
        name: String,
        output: String,
        target: Option<(u64, u64, (usize, usize))>,
        result: Result<String, String>,
    ) {
        let outcome = (|| -> Result<()> {
            let text = result.map_err(anyhow::Error::msg)?;
            match output.as_str() {
                "replace" | "insert" => {
                    let (doc, content, (start, end)) =
                        target.ok_or_else(|| anyhow::anyhow!("No document"))?;
                    let d = self
                        .documents
                        .get(&doc)
                        .ok_or_else(|| anyhow::anyhow!("The document was closed"))?;
                    if d.content_version() != content {
                        bail!("The document changed while {name} ran");
                    }
                    let cursor = self
                        .views
                        .values()
                        .find(|v| v.document == doc)
                        .map_or(0, |v| v.cursor);
                    self.documents
                        .get_mut(&doc)
                        .unwrap()
                        .replace(start, end, &text, cursor)?;
                    self.rebase_views(doc, start, end, text.len());
                    self.status = format!("{name} done");
                }
                "document" => {
                    let doc = self.id();
                    self.documents.insert(
                        doc,
                        crate::document::Document::inspection(format!("{name} · output"), text),
                    );
                    self.reveal_document(doc);
                    self.status = format!("{name} done");
                }
                "status" => {
                    self.status = format!(
                        "{name}: {}",
                        text.trim().lines().next().unwrap_or("(no output)")
                    );
                }
                _ => self.status = format!("{name} done"),
            }
            Ok(())
        })();
        if let Err(e) = outcome {
            self.status = format!("{name} failed: {e:#}");
        }
    }

    /// Catalog entries for tools.
    pub(crate) fn tool_catalog(&self) -> Vec<crate::commands::CommandInfo> {
        let editor = self.active_editor().is_some();
        self.tools
            .iter()
            .map(|t| {
                let needs_editor =
                    t.input != "none" || matches!(t.output.as_str(), "replace" | "insert");
                crate::commands::CommandInfo {
                    id: t.id.clone(),
                    name: format!("Tool: {}", t.name),
                    description: if t.description.is_empty() {
                        t.command.clone()
                    } else {
                        t.description.clone()
                    },
                    shortcut: t.key.clone(),
                    argument: String::new(),
                    enabled: editor || !needs_editor,
                    reason: if editor || !needs_editor {
                        String::new()
                    } else {
                        "Focus an editor".into()
                    },
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_load_with_ids_and_validate_modes() {
        let _env = crate::paths::TEST_ENV
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path());
        std::fs::create_dir_all(dir.path().join("slate")).unwrap();
        std::fs::write(
            dir.path().join("slate/tools.toml"),
            "[[tool]]\nname = \"Sort lines!\"\ncommand = \"sort\"\n\n[[tool]]\nname = \"Bad\"\ncommand = \"x\"\noutput = \"explode\"\n",
        )
        .unwrap();
        let root = dir.path().join("project");
        std::fs::create_dir_all(root.join(".slate")).unwrap();
        std::fs::write(
            root.join(".slate/tools.toml"),
            "[[tool]]\nname = \"Lint\"\ncommand = \"lint\"\ninput = \"document\"\noutput = \"status\"\n\n[[tool]]\nname = \"sort lines\"\ncommand = \"evil\"\n",
        )
        .unwrap();
        let tools = load(&root);
        let ids: Vec<&str> = tools.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["tool:sort-lines", "tool:lint"]);
        assert_eq!(
            tools[0].command, "sort",
            "a project tool cannot replace a personal one"
        );
        assert!(!tools[0].project && tools[1].project);
        assert_eq!(
            (tools[0].input.as_str(), tools[0].output.as_str()),
            ("selection", "replace")
        );
    }
}
