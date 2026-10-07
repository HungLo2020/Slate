//! A single discoverable action catalog for GUI controls, the TUI and bindings.
use crate::{App, Command};
use serde::Serialize;
#[derive(Clone, Serialize)]
pub struct CommandInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub shortcut: String,
    pub argument: String,
    pub enabled: bool,
    pub reason: String,
}
// IDs also retain the existing command-line spelling for scripting compatibility.
const ACTIONS: &[(&str, &str, &str, &str, &str)] = &[
    (
        "open",
        "Open file…",
        "Open a file or browse a directory",
        "Path",
        "any",
    ),
    (
        "new",
        "New document",
        "Create an untitled document",
        "",
        "any",
    ),
    (
        "save",
        "Save document",
        "Write the active document to disk",
        "",
        "edit",
    ),
    (
        "save-as",
        "Save document as…",
        "Save to a new path",
        "New file path",
        "edit",
    ),
    (
        "close",
        "Close document",
        "Close the active document; preserve unsaved work",
        "",
        "editor",
    ),
    (
        "discard-document",
        "Discard document…",
        "Discard unsaved changes and close",
        "",
        "editor",
    ),
    (
        "undo",
        "Undo",
        "Undo the last editing operation",
        "",
        "edit",
    ),
    (
        "redo",
        "Redo",
        "Restore the last undone operation",
        "",
        "edit",
    ),
    ("copy", "Copy selection", "Copy selected text", "", "text"),
    (
        "cut",
        "Cut selection",
        "Copy and remove selected text",
        "",
        "edit",
    ),
    ("paste", "Paste", "Insert clipboard text", "", "write"),
    (
        "select-all",
        "Select all",
        "Select the entire document",
        "",
        "editor",
    ),
    (
        "prompt-find",
        "Find…",
        "Search the active document",
        "",
        "editor",
    ),
    (
        "prompt-replace",
        "Replace…",
        "Find and replace text",
        "",
        "edit",
    ),
    (
        "prompt-goto",
        "Go to line…",
        "Jump to a document line",
        "",
        "editor",
    ),
    (
        "find-next",
        "Find next",
        "Select the next search result",
        "",
        "search",
    ),
    (
        "find-previous",
        "Find previous",
        "Select the previous search result",
        "",
        "search",
    ),
    (
        "indent",
        "Indent",
        "Indent the current line or selection",
        "",
        "edit",
    ),
    (
        "outdent",
        "Outdent",
        "Remove one indentation level",
        "",
        "edit",
    ),
    (
        "split-right",
        "Split pane right",
        "Create another view beside this pane",
        "",
        "any",
    ),
    (
        "split-down",
        "Split pane below",
        "Create another view below this pane",
        "",
        "any",
    ),
    (
        "split-terminal-right",
        "Split terminal right",
        "Create a shell beside this pane",
        "",
        "any",
    ),
    (
        "split-terminal-down",
        "Split terminal below",
        "Create a shell below this pane",
        "",
        "any",
    ),
    (
        "terminal",
        "New terminal",
        "Start another terminal session",
        "",
        "any",
    ),
    (
        "terminate-terminal",
        "Terminate terminal",
        "Stop the focused shell and its terminal",
        "",
        "terminal",
    ),
    (
        "files",
        "Show files",
        "Add a file browser to this pane",
        "",
        "any",
    ),
    (
        "git",
        "Show Git changes",
        "Add a Git view to this pane",
        "",
        "any",
    ),
    (
        "editor",
        "Show editor",
        "Add a document view to this pane",
        "",
        "any",
    ),
    (
        "close-pane",
        "Close pane",
        "Remove this pane from the layout",
        "",
        "pane",
    ),
    (
        "move-pane",
        "Move or swap pane…",
        "Swap with another pane",
        "Target pane number",
        "pane",
    ),
    (
        "next-pane",
        "Focus next pane",
        "Move keyboard focus to the next pane",
        "",
        "any",
    ),
    (
        "next-tab",
        "Next tab",
        "Activate the next view in this pane",
        "",
        "any",
    ),
    (
        "toggle-workspace",
        "Expand / collapse workspace",
        "Switch between the editor and the full workspace without closing views",
        "",
        "any",
    ),
    (
        "editor-only",
        "Show editor only",
        "Hide tool panes and keep their tabs and terminal sessions",
        "",
        "any",
    ),
    (
        "workspace",
        "Expand workspace",
        "Restore the full pane layout",
        "",
        "any",
    ),
    (
        "preset development",
        "Layout: three panes",
        "Files, editor and terminal side by side",
        "",
        "any",
    ),
    (
        "preset minimal",
        "Layout: editor only",
        "Focus on one editor pane",
        "",
        "any",
    ),
    (
        "preset bottom_terminal",
        "Layout: terminal below",
        "Place the terminal below the editor",
        "",
        "any",
    ),
    (
        "layout-save",
        "Save layout…",
        "Save the current pane arrangement",
        "Layout name",
        "any",
    ),
    (
        "layout-load",
        "Load layout…",
        "Restore a saved pane arrangement",
        "Layout name",
        "any",
    ),
    (
        "refresh",
        "Refresh files and Git",
        "Reload workspace files and repository changes",
        "",
        "any",
    ),
    (
        "stage",
        "Stage selected file",
        "Add working changes to the Git index",
        "",
        "git",
    ),
    (
        "unstage",
        "Unstage selected file",
        "Remove changes from the Git index",
        "",
        "git",
    ),
    (
        "diff",
        "Inspect selected changes",
        "Open a read-only diff or untracked-file preview",
        "",
        "git",
    ),
    (
        "stage-all",
        "Stage all changes",
        "Stage all working changes and untracked files in the repository",
        "",
        "unstaged",
    ),
    (
        "unstage-all",
        "Unstage all changes",
        "Remove all staged changes from the index; keep working files",
        "",
        "repository",
    ),
    (
        "commit",
        "Commit staged changes…",
        "Create a commit from staged changes",
        "Commit message",
        "repository",
    ),
    (
        "settings",
        "Settings…",
        "Configure indentation, appearance and editing",
        "",
        "any",
    ),
    (
        "settings-reload",
        "Reload settings",
        "Read settings.toml again",
        "",
        "any",
    ),
    (
        "quit",
        "Quit",
        "Exit after checking unsaved documents",
        "",
        "any",
    ),
    (
        "discard-quit",
        "Discard changes and quit…",
        "Discard all unsaved documents and exit",
        "",
        "any",
    ),
];
impl App {
    pub fn command_catalog(&self, query: &str) -> Vec<CommandInfo> {
        let query = query.trim().to_lowercase();
        let editor = self
            .active_editor()
            .map(|v| &self.documents[&self.views[&v].document]);
        let mut results = Vec::new();
        for &(id, name, description, argument, scope) in ACTIONS {
            let haystack = format!("{id} {name} {description}").to_lowercase();
            if !query.split_whitespace().all(|term| haystack.contains(term)) {
                continue;
            }
            let mut enabled = match scope {
                "editor" => editor.is_some(),
                "edit" => editor.is_some_and(|d| !d.read_only),
                "text" => editor.is_some() || self.focused_kind() == "terminal",
                "write" => {
                    editor.is_some_and(|d| !d.read_only) || self.focused_kind() == "terminal"
                }
                "terminal" => self.focused_kind() == "terminal",
                "search" => editor.is_some() && !self.search.query.is_empty(),
                "git" => {
                    self.focused_kind() == "git"
                        && self.selected_path().is_some()
                        && self.git_jobs == 0
                }
                "repository" => {
                    self.git_repository && self.git_jobs == 0 && self.git.iter().any(|e| e.staged)
                }
                "unstaged" => {
                    self.git_repository && self.git_jobs == 0 && self.git.iter().any(|e| !e.staged)
                }
                "pane" => !self.editor_only && self.layout.panes().len() > 1,
                _ => true,
            };
            if scope == "git" {
                if let Some(entry) = self.git.get(self.git_selected) {
                    if id == "stage" {
                        enabled &= !entry.staged;
                    }
                    if id == "unstage" {
                        enabled &= entry.staged;
                    }
                }
            }
            let shortcut = self
                .preferences
                .global_keys
                .iter()
                .chain(if self.focused_kind() == "terminal" {
                    self.preferences.terminal_keys.iter()
                } else {
                    self.preferences.editor_keys.iter()
                })
                .find(|(_, value)| value.as_str() == id)
                .map(|(key, _)| key.clone())
                .unwrap_or_default();
            let reason = if enabled {
                ""
            } else {
                match scope {
                    "edit" => "Focus an editable document",
                    "write" => "Focus an editable document or terminal",
                    "editor" => "Focus an editor",
                    "search" => "Search for text first",
                    "terminal" => "Focus a terminal",
                    "git" => "Select a Git change; wait for running operations",
                    "repository" => "Stage changes in a Git repository first",
                    "unstaged" => "No unstaged changes, or a Git operation is running",
                    "pane" if self.editor_only => "Expand the workspace first",
                    "pane" => "The final pane cannot be closed",
                    _ => "Focus an editor or terminal",
                }
            };
            results.push(CommandInfo {
                id: id.into(),
                name: name.into(),
                description: description.into(),
                argument: argument.into(),
                shortcut,
                enabled,
                reason: reason.into(),
            });
        }
        // Exact IDs appear first, so keyboard users can retain familiar short commands.
        results.sort_by_key(|c| {
            (
                c.id != query,
                !c.id.starts_with(&query),
                c.name.to_lowercase(),
            )
        });
        results
    }
    pub(super) fn invoke_action(&mut self, id: &str, argument: &str) -> anyhow::Result<()> {
        let action = self
            .command_catalog("")
            .into_iter()
            .find(|c| c.id == id)
            .ok_or_else(|| anyhow::anyhow!("Unknown action: {id}"))?;
        if !action.enabled {
            anyhow::bail!("{}", action.reason);
        }
        if id == "settings" {
            self.execute(Command::Prompt {
                kind: "settings".into(),
            })?;
        } else if !action.argument.is_empty() && argument.trim().is_empty() {
            self.execute(Command::Prompt { kind: id.into() })?;
        } else {
            self.command_line(&format!("{id} {argument}"));
        }
        Ok(())
    }
}
