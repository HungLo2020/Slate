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
        "open-folder",
        "Open folder…",
        "Browse a directory in the file pane",
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
        "save-all",
        "Save all",
        "Save every modified document",
        "",
        "any",
    ),
    (
        "reload",
        "Reload from disk",
        "Read the file again, discarding unsaved changes after confirmation",
        "",
        "file",
    ),
    (
        "set-encoding",
        "Set encoding…",
        "Choose the character encoding used when saving (UTF-8, UTF-16LE, windows-1252…)",
        "Encoding",
        "edit",
    ),
    (
        "reopen-encoding",
        "Reopen with encoding…",
        "Read the file again using another character encoding",
        "Encoding",
        "file",
    ),
    (
        "set-line-ending",
        "Set line endings…",
        "Save with LF, CRLF or CR line endings",
        "lf, crlf or cr",
        "edit",
    ),
    (
        "toggle-bom",
        "Toggle byte-order mark",
        "Add or remove a Unicode BOM when saving",
        "",
        "edit",
    ),
    (
        "toggle-read-only",
        "Toggle read-only",
        "Prevent or allow editing this document",
        "",
        "file",
    ),
    (
        "insert-file",
        "Insert file…",
        "Insert another file at the cursor",
        "Path",
        "edit",
    ),
    (
        "close",
        "Close tab",
        "Close the active file or terminal tab; preserve unsaved documents",
        "",
        "closable",
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
        "mark",
        "Set or unset mark",
        "Cursor movement extends the selection from the mark (nano)",
        "",
        "editor",
    ),
    (
        "cut-line",
        "Cut line",
        "Cut the current line or selection; repeated cuts collect lines",
        "",
        "edit",
    ),
    (
        "copy-line",
        "Copy line",
        "Copy the current line or selection",
        "",
        "editor",
    ),
    (
        "uncut",
        "Paste cut text",
        "Insert the most recently cut or copied text",
        "",
        "edit",
    ),
    (
        "justify",
        "Justify paragraph",
        "Reflow the paragraph or selection to the wrap column",
        "",
        "edit",
    ),
    (
        "spell-check",
        "Check spelling",
        "List misspelled words using aspell, hunspell or enchant",
        "",
        "editor",
    ),
    (
        "next-misspelling",
        "Next misspelling",
        "Select the next misspelled word",
        "",
        "editor",
    ),
    (
        "word-count",
        "Word count",
        "Count lines, words and characters in the document or selection",
        "",
        "editor",
    ),
    (
        "location",
        "Cursor position",
        "Report the line, column and character position",
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
        "stage-group",
        "Stage a change group…",
        "Stage all files in an unstaged change group",
        "Change group",
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
        "toggle-soft-wrap",
        "Toggle soft wrap",
        "Wrap long lines at the window edge",
        "",
        "any",
    ),
    (
        "toggle-line-numbers",
        "Toggle line numbers",
        "Show or hide the line-number gutter",
        "",
        "any",
    ),
    (
        "toggle-backup",
        "Toggle backups",
        "Keep the previous file as NAME~ when saving",
        "",
        "any",
    ),
    (
        "toggle-mouse",
        "Toggle mouse capture",
        "Let the outer terminal select and copy text with the mouse",
        "",
        "terminal-ui",
    ),
    (
        "suspend",
        "Suspend",
        "Stop Slate and return to the shell; resume with fg",
        "",
        "terminal-ui",
    ),
    (
        "toggle-whitespace",
        "Toggle whitespace markers",
        "Show spaces as · and tabs as →",
        "",
        "any",
    ),
    (
        "toggle-auto-reload",
        "Toggle automatic reload",
        "Reload unmodified documents when their file changes on disk",
        "",
        "any",
    ),
    (
        "toggle-minimap",
        "Toggle minimap",
        "Show a document overview beside editors",
        "",
        "graphical",
    ),
    (
        "zoom-in",
        "Zoom in",
        "Increase the editor font size",
        "",
        "graphical",
    ),
    (
        "zoom-out",
        "Zoom out",
        "Decrease the editor font size",
        "",
        "graphical",
    ),
    (
        "zoom-reset",
        "Reset zoom",
        "Use the default editor font size",
        "",
        "graphical",
    ),
    (
        "open-recent",
        "Open recent file…",
        "Reopen a recently used file (path or list number)",
        "Path or number",
        "any",
    ),
    (
        "clear-recent",
        "Clear recent files",
        "Forget the recently used files",
        "",
        "any",
    ),
    (
        "new-window",
        "New window",
        "Open another Slate window",
        "",
        "graphical",
    ),
    (
        "print",
        "Print…",
        "Print the active document",
        "",
        "graphical-editor",
    ),
    (
        "help",
        "Help",
        "Show the key bindings of the current keymap",
        "",
        "any",
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
        self.command_catalog_for(query, self.focus, None)
    }
    /// Describe a pane or row without changing the user's active focus/selection.
    pub fn command_catalog_for(
        &self,
        query: &str,
        pane: u64,
        selection: Option<usize>,
    ) -> Vec<CommandInfo> {
        let query = query.trim().to_lowercase();
        let view = self.layout.view(pane);
        let kind = match view {
            Some(crate::layout::View::Editor(_)) => "editor",
            Some(crate::layout::View::Terminal(_)) => "terminal",
            Some(crate::layout::View::Git) => "git",
            Some(crate::layout::View::Files) => "files",
            None => "",
        };
        let editor = match view {
            Some(crate::layout::View::Editor(id)) => {
                Some(&self.documents[&self.views[id].document])
            }
            _ => None,
        };
        let selected = self.git.get(selection.unwrap_or(self.git_selected));
        let mut results = Vec::new();
        for &(id, name, description, argument, scope) in ACTIONS {
            let haystack = format!("{id} {name} {description}").to_lowercase();
            if !query.split_whitespace().all(|term| haystack.contains(term)) {
                continue;
            }
            let mut enabled = match scope {
                "closable" => editor.is_some() || kind == "terminal",
                "editor" => editor.is_some(),
                "edit" => editor.is_some_and(|d| !d.read_only),
                "text" => editor.is_some() || kind == "terminal",
                "write" => editor.is_some_and(|d| !d.read_only) || kind == "terminal",
                "terminal" => kind == "terminal",
                "search" => editor.is_some() && !self.search.query.is_empty(),
                "git" => kind == "git" && selected.is_some() && self.git_jobs == 0,
                "repository" => {
                    self.git_repository && self.git_jobs == 0 && self.git.iter().any(|e| e.staged)
                }
                "unstaged" => {
                    self.git_repository && self.git_jobs == 0 && self.git.iter().any(|e| !e.staged)
                }
                "pane" => !self.editor_only && self.layout.panes().len() > 1,
                "file" => editor.is_some_and(|d| d.path.is_some() && d.label.is_none()),
                "terminal-ui" => self.terminal_frontend,
                "graphical" => !self.terminal_frontend,
                "graphical-editor" => !self.terminal_frontend && editor.is_some(),
                _ => true,
            };
            if scope == "git" {
                if let Some(entry) = selected {
                    if id == "stage" {
                        enabled &= !entry.staged;
                    }
                    if id == "unstage" {
                        enabled &= entry.staged;
                    }
                }
            }
            if matches!(id, "quit" | "discard-quit" | "save" | "save-as") {
                enabled &= self.pending_save.is_empty();
            }
            if id == "refresh" && kind == "git" {
                enabled &= self.git_jobs == 0;
            }
            let shortcut = self
                .preferences
                .global_keys
                .iter()
                .chain(
                    match kind {
                        "terminal" => Some(&self.preferences.terminal_keys),
                        "editor" => Some(&self.preferences.editor_keys),
                        _ => None,
                    }
                    .into_iter()
                    .flat_map(|keys| keys.iter()),
                )
                .find(|(key, value)| {
                    value.as_str() == id
                        && self
                            .preferences
                            .global_keys
                            .get(*key)
                            .is_none_or(|binding| binding.as_str() == id)
                })
                .map(|(key, _)| key.clone())
                .unwrap_or_default();
            let reason = if enabled {
                ""
            } else if matches!(id, "quit" | "discard-quit" | "save" | "save-as")
                && !self.pending_save.is_empty()
            {
                "Wait for pending file saves"
            } else if id == "refresh" && kind == "git" && self.git_jobs != 0 {
                "Wait for running Git operations"
            } else {
                match scope {
                    "closable" => "Focus an editor or terminal",
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
                    "file" => "Focus a document that has a file",
                    "terminal-ui" => "Only available in the terminal interface",
                    "graphical" => "Only available in the graphical interface",
                    "graphical-editor" => "Focus a document in the graphical interface",
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
        let command = self.resolve_action(id, argument)?;
        self.execute(command)
    }
    pub fn resolve_action(&self, id: &str, argument: &str) -> anyhow::Result<Command> {
        let action = self
            .command_catalog("")
            .into_iter()
            .find(|c| c.id == id)
            .ok_or_else(|| anyhow::anyhow!("Unknown action: {id}"))?;
        if !action.enabled {
            anyhow::bail!("{}", action.reason);
        }
        if id == "settings" {
            Ok(Command::Prompt {
                kind: "settings".into(),
            })
        } else if !action.argument.is_empty() && argument.trim().is_empty() {
            Ok(Command::Prompt { kind: id.into() })
        } else {
            self.resolve_command_line(&format!("{id} {argument}"))
                .ok_or_else(|| anyhow::anyhow!("Unknown or incomplete command: {id}"))
        }
    }
    /// Bare catalog actions request their argument prompt on every input route.
    /// Commands with supplied arguments retain the shared command-line syntax.
    pub fn resolve_command_input(&self, input: &str) -> anyhow::Result<Command> {
        let input = input.trim();
        if self
            .command_catalog("")
            .iter()
            .any(|action| action.id == input)
        {
            return self.resolve_action(input, "");
        }
        self.resolve_command_line(input).ok_or_else(|| {
            anyhow::anyhow!(
                "Unknown or incomplete command. Open Commands (F1) to search available actions."
            )
        })
    }
}

impl Command {
    /// The shared catalog identity of a resolved command, when it has one.
    pub fn action_id(&self) -> Option<String> {
        let id = match self {
            Command::New => "new",
            Command::Save => "save",
            Command::SaveAs { .. } => "save-as",
            Command::CloseDocument { force: false } => "close",
            Command::CloseDocument { force: true } => "discard-document",
            Command::Quit { force: false } => "quit",
            Command::Quit { force: true } => "discard-quit",
            Command::Undo => "undo",
            Command::Redo => "redo",
            Command::Copy => "copy",
            Command::Cut => "cut",
            Command::Paste { .. } => "paste",
            Command::SelectAll => "select-all",
            Command::Indent { outdent: false } => "indent",
            Command::Indent { outdent: true } => "outdent",
            Command::Prompt { kind } => {
                return Some(match kind.as_str() {
                    "find" | "replace" | "goto" => format!("prompt-{kind}"),
                    _ => kind.clone(),
                })
            }
            Command::ReloadSettings => "settings-reload",
            Command::EditorOnly => "editor-only",
            Command::ShowWorkspace => "workspace",
            Command::ToggleWorkspace => "toggle-workspace",
            Command::NewTerminal => "terminal",
            Command::TerminateTerminal => "terminate-terminal",
            Command::AddView { kind } => return Some(kind.clone()),
            Command::ClosePane => "close-pane",
            Command::FocusNext => "next-pane",
            Command::NextTab => "next-tab",
            Command::Preset { name } => return Some(format!("preset {name}")),
            Command::Refresh => "refresh",
            Command::GitStage { .. } => "stage",
            Command::GitUnstage { .. } => "unstage",
            Command::GitStageAll => "stage-all",
            Command::GitUnstageAll => "unstage-all",
            Command::GitStageGroup { .. } => "stage-group",
            Command::GitDiff { .. } => "diff",
            Command::GitCommit { .. } => "commit",
            Command::SaveAll { quit: false } => "save-all",
            Command::Action { name, .. } => return Some(name.clone()),
            _ => return None,
        };
        Some(id.into())
    }
}
