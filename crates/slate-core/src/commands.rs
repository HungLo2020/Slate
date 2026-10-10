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
/// A key chord as people write it: `F12`, `Ctrl+Space`, `Ctrl+Shift+[`.
pub fn display_chord(chord: &str) -> String {
    let (modifiers, key) = match chord.rfind('+') {
        // `Ctrl++` binds the plus key.
        Some(i) if i + 1 == chord.len() && i > 0 => (&chord[..i - 1], "+"),
        Some(i) => (&chord[..i], &chord[i + 1..]),
        None => ("", chord),
    };
    let mut parts: Vec<String> = modifiers
        .split('+')
        .filter(|m| !m.is_empty())
        .map(str::to_string)
        .collect();
    let shifted = match key {
        "{" => Some("["),
        "}" => Some("]"),
        "|" => Some("\\"),
        "_" => Some("-"),
        ":" => Some(";"),
        "\"" => Some("'"),
        "<" => Some(","),
        ">" => Some("."),
        "?" => Some("/"),
        _ => None,
    };
    let name = match (key, shifted) {
        (_, Some(base)) => {
            if !parts.iter().any(|p| p == "Shift") {
                parts.push("Shift".into());
            }
            base.to_string()
        }
        (" ", _) => "Space".into(),
        (k, _)
            if k.len() > 1 && k.starts_with('f') && k[1..].chars().all(|c| c.is_ascii_digit()) =>
        {
            k.to_uppercase()
        }
        (k, _) if k.chars().count() == 1 => k.to_uppercase(),
        (k, _) => {
            let mut c = k.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
                .replace("Pageup", "PageUp")
                .replace("Pagedown", "PageDown")
        }
    };
    parts.push(name);
    parts.join("+")
}

// IDs also retain the existing command-line spelling for scripting compatibility.
const ACTIONS: &[(&str, &str, &str, &str, &str)] = &[
    (
        "profile-save",
        "Save profile…",
        "Save this layout and preferences as a named profile",
        "Profile name",
        "any",
    ),
    (
        "profile-load",
        "Load profile…",
        "Apply a saved profile",
        "Profile name",
        "any",
    ),
    (
        "new-file",
        "New file…",
        "Create a file in the browsed folder",
        "File name",
        "files",
    ),
    (
        "new-folder",
        "New folder…",
        "Create a folder",
        "Folder name",
        "files",
    ),
    (
        "rename-file",
        "Rename…",
        "Rename the selected file or folder",
        "New name",
        "file-entry",
    ),
    (
        "trash-file",
        "Move to Trash…",
        "Move the selected entry to desktop Trash",
        "",
        "file-entry",
    ),
    (
        "toggle-folder",
        "Expand/collapse folder",
        "Toggle the selected explorer folder",
        "",
        "files",
    ),
    (
        "collapse-all-folders",
        "Collapse all folders",
        "Collapse every expanded explorer folder",
        "",
        "files",
    ),
    (
        "terminal-here",
        "Open terminal here",
        "Start a terminal in the selected folder, or the selected file's folder",
        "",
        "file-entry",
    ),
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
        "previous-tab",
        "Previous tab",
        "Activate the previous view in this pane",
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
        "open-recent-project",
        "Open recent project…",
        "Open a recent workspace by path or number",
        "Path or number",
        "any",
    ),
    (
        "search-settings",
        "Project search settings…",
        "Include/exclude globs and visibility controls",
        "",
        "any",
    ),
    (
        "support-report",
        "Support report",
        "Show versions, service state, paths and bounded diagnostic history",
        "",
        "any",
    ),
    (
        "diff-side-by-side",
        "Side-by-side diff",
        "Compare the selected Git file in two aligned views",
        "",
        "git",
    ),
    (
        "stage-hunk",
        "Stage hunk",
        "Stage the hunk at the cursor in a working diff",
        "",
        "diff-working",
    ),
    (
        "unstage-hunk",
        "Unstage hunk",
        "Unstage the hunk at the cursor in a staged diff",
        "",
        "diff-staged",
    ),
    (
        "accept-ours",
        "Conflict: accept ours",
        "Keep our side of the conflict at the caret",
        "",
        "editor",
    ),
    (
        "accept-theirs",
        "Conflict: accept theirs",
        "Keep their side of the conflict at the caret",
        "",
        "editor",
    ),
    (
        "accept-both",
        "Conflict: accept both",
        "Keep both sides of the conflict at the caret",
        "",
        "editor",
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
        "add-cursor-above",
        "Add cursor above",
        "Add a caret on the line above",
        "",
        "editor",
    ),
    (
        "add-cursor-below",
        "Add cursor below",
        "Add a caret on the line below",
        "",
        "editor",
    ),
    (
        "add-next-occurrence",
        "Add next occurrence",
        "Select the word, then add the next occurrence as another caret",
        "",
        "editor",
    ),
    (
        "select-all-occurrences",
        "Select all occurrences",
        "Put a caret on every occurrence of the selection",
        "",
        "editor",
    ),
    (
        "go-to-bracket",
        "Go to matching bracket",
        "Jump to the bracket that pairs with the one at the cursor",
        "",
        "editor",
    ),
    (
        "fold",
        "Fold",
        "Fold the indented region at the cursor",
        "",
        "editor",
    ),
    (
        "unfold",
        "Unfold",
        "Unfold the region at the cursor",
        "",
        "editor",
    ),
    (
        "toggle-fold",
        "Toggle fold",
        "Fold or unfold the region at the cursor",
        "",
        "editor",
    ),
    (
        "fold-all",
        "Fold all",
        "Fold every top-level region",
        "",
        "editor",
    ),
    (
        "unfold-all",
        "Unfold all",
        "Unfold every region",
        "",
        "editor",
    ),
    (
        "quick-open",
        "Go to file…",
        "Find a file in the workspace by name",
        "",
        "any",
    ),
    (
        "go-to-symbol",
        "Go to symbol…",
        "List functions, types and headings in the document",
        "",
        "editor",
    ),
    (
        "search-in-files",
        "Search in files…",
        "Search the whole workspace (respects .gitignore)",
        "",
        "any",
    ),
    (
        "replace-in-files",
        "Replace in files…",
        "Replace project search results",
        "",
        "any",
    ),
    (
        "undo-replace-in-files",
        "Undo replace in files",
        "Restore the files the last replace in files rewrote on disk",
        "",
        "any",
    ),
    (
        "open-documents",
        "Open documents…",
        "Switch to another open document",
        "",
        "any",
    ),
    (
        "problems",
        "Problems…",
        "List errors and warnings reported by language servers and tasks",
        "",
        "any",
    ),
    (
        "go-back",
        "Go back",
        "Return to the position before the last jump",
        "",
        "any",
    ),
    ("go-forward", "Go forward", "Redo a go-back", "", "any"),
    (
        "signature-help",
        "Signature help",
        "Show function parameters at the caret",
        "",
        "editor",
    ),
    (
        "select-rectangle",
        "Rectangular selection",
        "Select the columns between the selection's corners; Alt+Shift+drag also selects a block",
        "",
        "editor",
    ),
    (
        "hover",
        "Show hover information",
        "Show the language server's information for the symbol at the cursor",
        "",
        "editor",
    ),
    (
        "go-to-definition",
        "Go to definition",
        "Jump to where the symbol at the cursor is defined",
        "",
        "editor",
    ),
    (
        "find-references",
        "Find references",
        "List every use of the symbol at the cursor",
        "",
        "editor",
    ),
    (
        "rename-symbol",
        "Rename symbol…",
        "Rename the symbol at the cursor across the workspace",
        "",
        "edit",
    ),
    (
        "code-actions",
        "Code actions…",
        "Quick fixes and refactorings for the cursor or selection",
        "",
        "edit",
    ),
    (
        "format-document",
        "Format document",
        "Format with the language server or the language's formatter",
        "",
        "edit",
    ),
    (
        "trigger-completion",
        "Complete",
        "Suggest completions at the cursor",
        "",
        "edit",
    ),
    (
        "workspace-symbols",
        "Workspace symbols…",
        "Find a symbol anywhere in the workspace",
        "",
        "any",
    ),
    (
        "next-problem",
        "Next problem",
        "Move to the next error or warning in the document",
        "",
        "editor",
    ),
    (
        "previous-problem",
        "Previous problem",
        "Move to the previous error or warning",
        "",
        "editor",
    ),
    (
        "restart-language-servers",
        "Restart language servers",
        "Stop language servers; they start again for open documents",
        "",
        "any",
    ),
    (
        "run-task",
        "Run task…",
        "Run a build, test or project task",
        "",
        "any",
    ),
    (
        "run-build-task",
        "Run build task",
        "Run the default build task and collect its problems",
        "",
        "any",
    ),
    ("stop-task", "Stop tasks", "Stop running tasks", "", "tasks"),
    (
        "debug-start",
        "Start debugging",
        "Debug a program (launch.toml or a path); continues when paused",
        "",
        "any",
    ),
    (
        "debug-continue",
        "Continue",
        "Resume the paused program",
        "",
        "paused",
    ),
    (
        "debug-step-over",
        "Step over",
        "Run to the next line",
        "",
        "paused",
    ),
    (
        "debug-step-into",
        "Step into",
        "Step into the call on this line",
        "",
        "paused",
    ),
    (
        "debug-step-out",
        "Step out",
        "Run until the current function returns",
        "",
        "paused",
    ),
    (
        "debug-pause",
        "Pause",
        "Pause the running program",
        "",
        "running",
    ),
    (
        "debug-stop",
        "Stop debugging",
        "End the debug session and the program",
        "",
        "debugging",
    ),
    (
        "debug-evaluate",
        "Evaluate…",
        "Evaluate an expression in the paused frame",
        "",
        "paused",
    ),
    (
        "toggle-breakpoint",
        "Toggle breakpoint",
        "Set or remove a breakpoint on the current line",
        "",
        "editor",
    ),
    (
        "request-trust",
        "Trust this folder…",
        "Ask to trust the folder Git and project tools need",
        "",
        "any",
    ),
    (
        "clear-breakpoints",
        "Remove all breakpoints",
        "Clear every breakpoint in every file",
        "",
        "any",
    ),
    (
        "breakpoints",
        "Breakpoints…",
        "List breakpoints and jump to one",
        "",
        "any",
    ),
    (
        "debug-call-stack",
        "Call stack…",
        "Choose the frame whose locals are shown and where expressions evaluate",
        "",
        "paused",
    ),
    (
        "language-servers",
        "Language servers",
        "Show running language servers, missing ones and their messages",
        "",
        "any",
    ),
    (
        "trust-workspace",
        "Trust workspace",
        "Allow language servers, tasks, formatters and Git hooks from this folder to run",
        "",
        "untrusted",
    ),
    (
        "restrict-workspace",
        "Restrict workspace",
        "Stop running project tools from this folder (restricted mode)",
        "",
        "trusted",
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
        self.catalog(query, pane, selection, None)
    }
    /// One action of the focused pane's catalog, without describing the
    /// others.
    pub fn command_info(&self, id: &str) -> Option<CommandInfo> {
        self.catalog("", self.focus, None, Some(id))
            .into_iter()
            .next()
    }
    fn catalog(
        &self,
        query: &str,
        pane: u64,
        selection: Option<usize>,
        only: Option<&str>,
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
        let selected = self.git.entries.get(selection.unwrap_or(self.git.selected));
        let mut results = Vec::new();
        for &(id, name, description, argument, scope) in ACTIONS {
            if only.is_some_and(|only| only != id) {
                continue;
            }
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
                "git" => kind == "git" && selected.is_some() && self.git.jobs == 0,
                "repository" => {
                    self.git.repository
                        && self.git.jobs == 0
                        && self.git.entries.iter().any(|e| e.staged)
                }
                "unstaged" => {
                    self.git.repository
                        && self.git.jobs == 0
                        && self.git.entries.iter().any(|e| !e.staged)
                }
                "pane" => !self.editor_only && self.layout.panes().len() > 1,
                "file" => editor.is_some_and(|d| d.path.is_some() && d.label.is_none()),
                "terminal-ui" => self.terminal_frontend,
                "graphical" => !self.terminal_frontend,
                "graphical-editor" => !self.terminal_frontend && editor.is_some(),
                "diff-working" => self.can_stage_diff_hunk(false),
                "diff-staged" => self.can_stage_diff_hunk(true),
                "files" => kind == "files",
                "file-entry" => {
                    kind == "files"
                        && self
                            .files
                            .get(selection.unwrap_or(self.selected))
                            .is_some_and(|e| e.name != "..")
                }
                "trusted" => self.trusted(),
                "debugging" => self.debugging(),
                "paused" => self.debug_paused(),
                "running" => self.debugging() && !self.debug_paused(),
                "tasks" => self.tasks_running(),
                "untrusted" => !self.trusted(),
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
                enabled &= self.saves.pending_save.is_empty();
            }
            if id == "refresh" && kind == "git" {
                enabled &= self.git.jobs == 0;
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
                .or_else(|| {
                    self.preferences
                        .debug_keys
                        .iter()
                        .find(|(_, value)| value.as_str() == id)
                })
                .map(|(key, _)| display_chord(key))
                .unwrap_or_default();
            let reason = if enabled {
                ""
            } else if matches!(id, "quit" | "discard-quit" | "save" | "save-as")
                && !self.saves.pending_save.is_empty()
            {
                "Wait for pending file saves"
            } else if id == "refresh" && kind == "git" && self.git.jobs != 0 {
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
                    "trusted" => "The workspace is already restricted",
                    "debugging" => "Start debugging first",
                    "paused" => "Pause the program first (or start debugging)",
                    "running" => "The program is not running",
                    "tasks" => "No task is running",
                    "untrusted" => "The workspace is already trusted",
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
        for tool in self.tool_catalog() {
            if only.is_some_and(|only| only != tool.id) {
                continue;
            }
            let haystack = format!("{} {} {}", tool.id, tool.name, tool.description).to_lowercase();
            if query.split_whitespace().all(|term| haystack.contains(term)) {
                results.push(tool);
            }
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
        let command = self.resolve_action(id, argument)?.confirming_discard();
        self.execute(command)
    }
    pub fn resolve_action(&self, id: &str, argument: &str) -> anyhow::Result<Command> {
        let action = self
            .command_info(id)
            .ok_or_else(|| anyhow::anyhow!("Unknown action: {id}"))?;
        self.resolve_info(action, argument)
    }
    fn resolve_info(&self, action: CommandInfo, argument: &str) -> anyhow::Result<Command> {
        let id = action.id.as_str();
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
        if let Some(action) = self.command_info(input) {
            return self.resolve_info(action, "");
        }
        self.resolve_command_line(input).ok_or_else(|| {
            anyhow::anyhow!(
                "Unknown or incomplete command. Open Commands (F1) to search available actions."
            )
        })
    }
}

impl Command {
    /// Discarding commands picked, typed or bound in the shared input routes
    /// ask first: the unforced form opens the unsaved-changes prompt when
    /// there is something to lose, and only its acceptance discards. Frontends
    /// with their own dialogs dispatch the forced command once confirmed.
    pub(crate) fn confirming_discard(self) -> Self {
        match self {
            Command::Quit { force: true } => Command::Quit { force: false },
            Command::CloseDocument { force: true } => Command::CloseDocument { force: false },
            command => command,
        }
    }
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

#[cfg(test)]
mod chord_tests {
    use super::display_chord;

    #[test]
    fn chords_display_as_people_write_them() {
        assert_eq!(display_chord("f12"), "F12");
        assert_eq!(display_chord("Shift+f12"), "Shift+F12");
        assert_eq!(display_chord("Ctrl+ "), "Ctrl+Space");
        assert_eq!(display_chord("Ctrl+p"), "Ctrl+P");
        assert_eq!(display_chord("Ctrl+{"), "Ctrl+Shift+[");
        assert_eq!(display_chord("Ctrl+Alt+up"), "Ctrl+Alt+Up");
        assert_eq!(display_chord("Ctrl+pagedown"), "Ctrl+PageDown");
        assert_eq!(display_chord("Ctrl++"), "Ctrl++");
        assert_eq!(display_chord("Alt+Shift+n"), "Alt+Shift+N");
    }
}
