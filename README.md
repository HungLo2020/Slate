# Slate

A Rust editor with a Qt Quick GUI and a terminal UI sharing the same editor
core: a nano replacement in the terminal, a Kate/gedit-style desktop editor,
and a workspace with terminals and Git.

## Build and launch

Install Rust **1.89 or newer**, a C++17 compiler, CMake, and Qt 6
Core/Gui/Widgets/PrintSupport/Qml/Quick development packages. GUI runtime packages
must include Qt Quick Controls, Layouts, Templates, Window, QtQml, Models and
WorkerScript modules. No KDE frameworks are needed: the GUI follows any
desktop's Qt palette and icon theme (on Plasma, the KDE platform integration
supplies Breeze colours, icons and file dialogs). Git is used for the Git pane.
Your shell is taken from `SHELL`, falling back to `/bin/sh`.

CMake locates Qt (set `CMAKE_PREFIX_PATH` or `Qt6_DIR` for a non-system
installation) and tells Cargo which Qt libraries to link, so the build does not
depend on pkg-config. Slate does not download or vendor Qt.

File and terminal tabs have a **×** close button in the GUI and a clickable **[x]**
in the TUI. Closing a terminal tab stops that session and its shell; other sessions
keep running. **Ctrl+W** closes the active editor tab in either frontend. Closing
a shared document's tab leaves its other views intact; closing its last view
asks before discarding unsaved changes. Cancel to save first. In the TUI, press
**D** to discard or **Escape / Enter** to cancel. Closing the final editor tab
leaves an empty editor. On narrow tab bars, use the GUI's tab menu or **F7** in
the TUI to select hidden tabs.

The GUI uses Qt's native file dialogs for **Open File**, **Open Folder**, and
**Save As**. On Plasma, the desktop's Qt platform integration (`plasma-integration`
on Debian/Ubuntu) supplies KDE's file picker, Places sidebar and overwrite
confirmation. Other desktops use their available Qt platform dialog. Slate keeps
file I/O in the shared Rust backend and currently supports local files.
**Ctrl+O** opens a file, **Ctrl+Shift+O** opens a folder, and **Ctrl+Shift+S** saves
as a new path. Saving an untitled document opens Save As automatically.

GUI menus, pane menus, Git controls and the command palette share command
availability with configured shortcuts. **Ctrl+Q** (or its configured replacement)
and **File → Quit** use the same unsaved-changes confirmation; Cancel preserves
all edits. Raw palette commands such as `:discard-quit` also require confirmation.
Configured global shortcuts work in tool input fields as well as the editor.

```bash
cargo build --release                     # builds slate (terminal) and slate-gui
./target/release/slate .                  # terminal mode
./target/release/slate --gui .            # graphical mode (runs slate-gui)
./target/release/slate src/main.rs        # editor only
./target/release/slate --gui --editor-only .  # GUI editor only
./target/release/slate --workspace src/main.rs # file in the full TUI workspace
cargo build --release -p slate            # terminal executable only, no Qt needed
```

Development launchers build the release executable before starting it:

```bash
python3 DevUtils/RunGui.py                # GUI
python3 DevUtils/RunTui.py                # TUI in a new terminal window
python3 DevUtils/RunGui.py src/main.rs    # open a file; either launcher accepts a path
```

The launchers find the repository relative to their own location, default to
opening it, and accept Slate flags such as `--fresh`. Explicit relative paths
refer to the directory where the script was invoked. `RunTui.py` requires a
graphical desktop and an installed terminal emulator; it prefers Konsole and
never runs Slate in the terminal used to invoke the script.

Install both executables (`--tui-only` installs just `slate`):

```bash
./scripts/install.sh "$HOME/.local"
slate .
slate-gui .
```

`slate` is the terminal interface and does not link Qt, so it runs on servers,
over SSH and on minimal systems. `slate-gui` is the graphical interface; `slate
--gui` hands over to it, and `slate-gui --tui` runs the terminal interface.

## Using Slate as a nano replacement

```bash
slate notes.txt               # a missing file is created on first save
slate +42 src/main.rs         # start on line 42 (+LINE,COLUMN or +LINE:COLUMN)
slate a.rs b.rs               # several files open as tabs
git log | slate -             # edit standard input; saving asks for a path
slate -v /etc/fstab           # read-only view
EDITOR=slate git commit       # blocks until you quit, exits 0
```

Quitting with unsaved changes asks **Y** (save all and quit), **N** (discard
and quit) or **C**/Escape (cancel). Choose **Keymap → nano** in Settings (or set
`keymap = "nano"`) for nano's bindings: ^O write out, ^X exit, ^W where is,
^\\ replace, ^K cut line (repeated cuts collect lines), ^U paste, ^6/M-A mark,
^J justify, ^T spell check, ^_ go to line, ^C cursor position, M-D word count,
^R insert file, ^G help, ^Z suspend, M-U/M-E undo/redo, M-S soft wrap, M-N line
numbers and M-M mouse capture. The bottom bar shows nano-style hints.

Files keep their format. Slate reads UTF-8 (with or without a BOM), UTF-16 with a
BOM, and legacy 8-bit files (opened as windows-1252, or any encoding through
**Reopen with encoding**). LF, CRLF and CR line endings are preserved; mixed files
are reported and saved with their dominant ending. **Set encoding** and **Set line
endings** change how a document is saved. Pasted text and text from other
programs is normalized, so stray carriage returns never reach the file. Binary
files (containing NUL bytes) are refused.

Saving keeps file metadata: permissions, owner and group (when running as root),
extended attributes and ACLs, and hard links. Files in directories you cannot
write are updated in place. Saving a read-only file asks first; saving a file
you have no permission to write offers **sudo** (terminal) or **pkexec** (GUI).
With `backup = true`, the previous version is kept as `NAME~`. When the terminal
is closed or Slate receives SIGTERM, unsaved buffers of individually opened
files are written to `NAME.save`, as nano does.

The terminal interface uses the outer terminal's colour depth: 24-bit colour with
`COLORTERM=truecolor`, otherwise 256 or 16 colours, and none with `NO_COLOR`. It
requests the kitty keyboard protocol for unambiguous Ctrl/Shift keys and decodes
legacy terminals' control bytes (Ctrl+], ^\\, ^_, and ^H when the terminal's erase
key is ^H). Copy and paste use the desktop clipboard through `wl-copy`/`xclip`/
`xsel` when a display is available, and OSC 52 otherwise. **Toggle mouse capture**
leaves selection to the outer terminal. Soft wrap (**Alt+Z**, or nano's M-S) wraps
long lines at word boundaries; the cursor moves by screen rows.

## The desktop editor

`slate-gui FILE…` opens files in the running window, as Kate and gedit do; the
desktop entry passes every selected file (`%F`). Directories, `slate-gui -`
and `--new-instance` start a new window, and **File → New Window** opens one.
Drop files from a file manager onto the window to open them. **File → Open
Recent** lists the last 20 files, and **File → Print** prints the document.

The title bar shows the document, its folder and `*` for unsaved changes. Status
messages and errors (save failures, conflicts, refused files) appear in the
status bar; errors in red. Files changed by other programs reload automatically
when unmodified; a modified document asks whether to reload it or keep your
version (the next save replaces the file). Deleted files are reported and must be
saved or discarded.

The editor supports double-click word and triple-click line selection, a
blinking caret following the desktop's cursor flash time, smooth touchpad and
wheel scrolling, horizontal scrolling (Shift+wheel or a tilt wheel), a
horizontal scrollbar and a minimap that scrolls on click or drag. **View** toggles
word wrap, whitespace markers, line numbers and the minimap, and zooms
(**Ctrl+=**, **Ctrl+-**, **Ctrl+0**, or Ctrl+wheel). Choose the font in Settings.
**Edit → Document** sets the encoding and line endings, reopens a file with
another encoding, and toggles read-only mode. Right-click the editor for Undo,
Redo, Cut, Copy, Paste and Select All; the caret moves to the click unless text
is selected.

Input methods show their own composition styling and caret. Shortcuts work on
non-Latin keyboard layouts (by physical key), and screen readers get the
editor's text, caret and selection through Qt's accessibility interface. Logging
out keeps unsaved work (recovery checkpoints or `NAME.save` files); SIGTERM and
SIGHUP quit through the same path. Qt options such as `-platform`, `-style` and
`-reverse` pass through to Qt.

## Using Slate as an IDE

The IDE features work the same way in both interfaces. Every action is also in
the command palette (F1).

### Workspace trust

Opening a folder never runs code from it. Language servers, tasks, formatters,
debugging, project tools and Git wait until you trust the folder. Git is
included because a repository's own configuration (filters, fsmonitor, hooks)
can name programs to run, so the Git pane shows **Trust Folder…** instead of
changes until you trust the repository. The first action that needs trust
asks; you can also use **Trust workspace**, **Restrict workspace** and
**Trust this folder…** from the palette. The status bar shows **Restricted** until
you do.

Trust is decided per folder, not per window: a language server or formatter
for a file in another folder asks about that folder, and a repository that
contains the workspace asks about the repository. A single opened file's folder
is trusted for the session only, so editing a download does not trust all of
Downloads. Trusted folders, and everything inside them, are listed one per line
in `~/.config/slate/trusted-folders`; a line starting with `!` restricts a
folder inside a trusted one, and the most specific line wins.

### Language servers

Slate starts a language server for each document whose language has one
installed: rust-analyzer, pyright/pylsp/jedi, typescript-language-server,
clangd, gopls, bash-language-server, lua-language-server, marksman, zls,
jdtls, solargraph, taplo, nil and the vscode-* JSON/HTML/CSS servers. Add or
override servers and formatters in `~/.config/slate/languages.toml`:

```toml
[[language]]
name = "Python"                   # syntax name shown in the status bar
extensions = ["py"]               # optional: match by extension instead
server = ["pyright-langserver", "--stdio"]
formatter = ["black", "-q", "-"]  # reads standard input, writes standard output
roots = ["pyproject.toml"]
```

Edits are sent to the server as they happen. Errors and warnings are underlined
in the editor, marked in the gutter and counted in the status bar; the message
under the caret is shown there too. Completion opens after two typed characters
or a trigger character such as `.` (setting **Complete while typing**): Up/Down
to choose, Tab or Enter to accept (setting **Enter accepts a completion**),
Escape to close. Snippet completions put the caret in their first field. You
can also show hover information (Alt+H, or rest the mouse on a word in the
desktop editor), go to a definition, find references, rename a symbol across
the workspace, apply code actions, format the document, and list the
document's or workspace's symbols. Renames and code actions open the files they
change as unsaved tabs, so every change can be reviewed and undone; edits
computed for an older version of a file, or for files outside the workspace,
are refused. **Format on save** is a setting. **Language servers** lists the
running servers and their progress. Files without a server get an outline from
their syntax grammar.

### Finding things

| Key | Action |
| --- | --- |
| Ctrl+P | Go to file (fuzzy, respects `.gitignore`) |
| Ctrl+Shift+F | Search in files; Alt+C case, Alt+W whole word, Alt+R regex, Ctrl+H replace |
| Ctrl+R | Go to symbol in the document |
| Ctrl+T | Workspace symbols |
| Ctrl+E | Open documents |
| Alt+Left / Alt+Right | Go back / forward after a jump |
| F12 / Shift+F12 | Go to definition / find references |
| Alt+H | Hover information |
| F2 | Rename symbol |
| Ctrl+. or Alt+Enter | Code actions |
| Ctrl+Shift+I or Alt+Shift+F | Format document |
| Ctrl+Space | Complete |
| Ctrl+F8 / Ctrl+Shift+F8 | Next / previous problem |

Project search runs in the background as you type, searches unsaved documents
as edited, and skips binary and ignored files. Results can be clicked, and long
lists page in as you scroll. Replace in files changes open documents in the
editor and rewrites the others. It skips files that changed since the search
and files with mixed line endings, and says so. **Undo replace in files** puts
back the files it rewrote on disk, unless they changed again since.

These keys are in the default keymap. The finding keys also work from the file
and Git panes, but never in a terminal, so shells keep Ctrl+P, Ctrl+E and
Alt+arrows.

### Editing

| Key | Action |
| --- | --- |
| Ctrl+D | Select the word, then add its next occurrence as another caret |
| Ctrl+Shift+L or Alt+Shift+L | A caret on every occurrence |
| Ctrl+Alt+Up / Down | Add a caret above / below |
| Alt+click | Add or remove a caret |
| Escape | Back to one caret |
| Ctrl+Shift+[ / Ctrl+Shift+] or Alt+- / Alt+= | Fold / unfold the indented region (or click ▾/▸ in the gutter) |
| Ctrl+Tab / Ctrl+Shift+Tab (or Ctrl+PageDown / PageUp) | Next / previous tab |
| Ctrl+N | New document |
| Shift+F1 | Keyboard help |
| Ctrl+6 | Go to the matching bracket |
| Tab after a snippet prefix | Expand a snippet; Tab / Shift+Tab move between its fields |

With several carets, typing, deleting, Enter, Tab, paste (one line per caret when
the counts match), copy and cut act at every caret and undo as one step.
Brackets and quotes close automatically (setting **Close brackets and quotes**),
and a selection typed over with a bracket is wrapped. Enter keeps the
indentation and indents after an opening bracket or a trailing colon. A
closing bracket typed on a blank line lines up with its opener. The bracket
pair at the caret is highlighted. Built-in snippets cover Rust, Python,
JavaScript, TypeScript, C, C++, Go and shell. Add your own in
`~/.config/slate/snippets.toml`:

```toml
[[snippet]]
prefix = "todo"
body = "// TODO($1): $0"
language = "Rust"      # optional
```

### Tasks and problems

**Run build task** (Ctrl+Shift+B or Alt+Shift+B) and **Run task…** run tasks.
Tasks come from `.slate/tasks.toml`, or are detected for Cargo, Make, CMake, Go,
npm scripts and pytest. Output streams into a read-only document (one per task,
reused on the next run) that opens beside the editor without taking focus, and
a problem matcher (`gcc`, `rustc`, `tsc`, `python`, `generic`) adds the errors
and warnings to **Problems**. **Stop tasks** ends a task and everything it
started; tasks still running when Slate quits are stopped too.

```toml
[[task]]
name = "build"
command = "make -j"
group = "build"
matcher = "gcc"
```

### Debugging

**Start debugging** (F5) runs a program under GDB's Debug Adapter Protocol
mode (`gdb -i dap`), or another adapter. Configure programs in
`.slate/launch.toml`:

```toml
[[launch]]
name = "app"
program = "build/app"
args = ["--verbose"]
```

**Toggle breakpoint** is Ctrl+F9; breakpoints show as ◆ in the gutter, move
with edits and are remembered with the workspace. **Breakpoints…** lists them
and **Remove all breakpoints** clears them. While debugging, F9 toggles
breakpoints, F10 steps over, F11 steps in, Shift+F11 steps out, F5 continues and
Shift+F5 stops (Alt+Shift+N/I/O/C step over, in, out and continue in terminals
that do not send function keys). These keys are the `debug_keys` table in
`settings.toml`. The editor follows the paused line (▶ in the gutter). A Debug
document shows the call stack, local variables and program output;
**Call stack…** switches to another frame, and **Evaluate…** evaluates an
expression in the selected frame. The desktop editor has a **Run** menu and,
while debugging, continue/step/stop buttons in the status bar. The last launch
configuration is remembered for the next **Start debugging**.

### External tools

Commands in `~/.config/slate/tools.toml` (and, in trusted folders,
`.slate/tools.toml`) appear in the palette as "Tool: NAME":

```toml
[[tool]]
name = "Sort lines"
command = "sort"
input = "selection"   # selection, document or none
output = "replace"    # replace, insert, document, status or none
key = "Alt+s"
```

Tools run with `sh -c` and get `SLATE_FILE`, `SLATE_LINE`, `SLATE_COLUMN`,
`SLATE_SELECTION` and `SLATE_ROOT`.

### Terminals

A terminal whose shell exits closes its tab, and the exit code is shown when
it is not zero. Programs can set the tab title. Programs can copy to the
clipboard (OSC 52) only with the **Terminal programs may set the clipboard**
setting (`terminal_clipboard`), since anything printed to a terminal (a `cat`
of a downloaded file, say) could otherwise replace what you copied. Pasted text
has control characters removed. The **Terminal scrollback lines** setting
controls history.

## Startup and settings

Opening a file defaults to just the editor; opening a directory, or launching
without a path, defaults to the full workspace. `--editor-only` and `--workspace`
override those defaults independently of `--gui`/`--tui`. If both layout flags
are supplied, the last one wins.

Use **Settings** in either command palette, or **Menu → Workspace → Settings** in
the GUI, to configure **Opening a file** and **Opening a directory** separately.
Settings are saved automatically to `$XDG_CONFIG_HOME/slate/settings.toml`
(normally `~/.config/slate/settings.toml`). The equivalent TOML fields are:

```toml
file_startup = "editor-only"
directory_startup = "workspace"
editor_theme = "dark"     # auto, dark, light
terminal_theme = "dark"   # auto, dark, light, editor
keymap = "default"        # or "nano"
soft_wrap = false
wrap_column = 80          # justify and hard wrap
hard_wrap = false
backup = false            # keep NAME~ on save
file_recovery = false     # recover unsaved single files after a crash
tui_mouse = true
terminal_clipboard = false          # let terminal programs set the clipboard (OSC 52)
complete_while_typing = true
accept_completion_on_enter = true
```

Editor and terminal themes are independent. Their dark defaults use charcoal
backgrounds (`#1b1e26` and `#14171c`) while GUI menus and dialogs keep the desktop
palette. `auto` follows the desktop palette in the GUI and uses a dark fallback
in the TUI. Terminal `editor` follows the editor's colors, including selection.
Terminal ANSI colors adapt to light/dark backgrounds; explicit application RGB
colors are preserved. `set theme` remains an alias for the editor theme.
Older explicit light/dark settings keep their editor choice.

In-place saves (hard links or unwritable parent directories) first persist a
private original under `$XDG_STATE_HOME/slate/save-recovery`. A failed write
restores it; if restoration fails, the error names the retained recovery file.
After abnormal termination, originals left there can be recovered manually.
Administrator saves use the same checked save path instead of `tee`.

GUI invocations with `--view`, `--fresh`, `--editor-only`, or `--workspace` open
a separate window, so their options cannot be lost when another window exists.

Keymaps gain new default bindings when Slate adds them, without changing keys
you set. A configuration file that does not parse (`settings.toml`,
`languages.toml`, `tools.toml`, `snippets.toml`, `layouts.toml`,
`.slate/tasks.toml`, `.slate/launch.toml`) is reported in the status bar instead
of being ignored, and Slate does not overwrite it.

Press **F10** to expand/collapse the workspace. The GUI also has a **Workspace** /
**Editor only** toolbar button. This keeps the complete split layout, open tabs,
unsaved edits, cursor positions and running shells. Editor-only startup launches
no hidden shell, even when recovering a previous workspace; expansion starts
remembered shells in their saved directories. Startup settings apply on the next
launch, and recovery does not override the chosen startup mode.

GUI Settings offers direct controls; TUI Settings uses ↑/↓ to select and ←/→ or
Enter to change a value. Ctrl+, opens Settings where the frontend/outer terminal
supports that shortcut. Keybindings, including F10, remain configurable in
`settings.toml`. Recovery data stays under `$XDG_STATE_HOME/slate/workspaces`
(normally `~/.local/state/slate/workspaces`). Empty or relative XDG directory
values use the standard home-directory defaults.

## Debian packages and MattPackages publication

Slate follows Basalt's MattPackages workflow. On an amd64 Debian/Ubuntu build
host with Rust 1.89+ and Python 3.11+, install the distro-provided dependencies
with `bash DevUtils/InstallDependencies.sh`, then:

```bash
bash DevUtils/Build.sh
python3 scripts/deb-smoke.py builds/slate_0.1.1_amd64.deb
sudo apt install ./builds/slate_0.1.1_amd64.deb
```

The package includes `/usr/bin/slate`, `/usr/bin/slate-gui`, the desktop entry,
icon and README. Only the terminal executable's libraries are hard dependencies;
Qt, the QML modules, SVG plugins and Git are recommended, so `apt install
--no-install-recommends slate` installs a Qt-free terminal editor. It does not
bundle Qt or require an exact Qt runtime version.

The application icon comes from `resources/slate.svg`, embedded in the GUI and
installed into the standard hicolor icon theme for the desktop launcher.

The manual **Build Linux amd64 DEB** workflow builds in Ubuntu 26.04, matching
Basalt's target distribution, and keeps the
package as a workflow artifact. A locally built package targets the libraries on
that build host, so it may require newer libraries than older distributions have.

Publishing uses the same authoritative repository manager as Basalt:

```bash
python3 DevUtils/PublishMattOSPackage.py doctor
python3 DevUtils/PublishMattOSPackage.py publish --dry-run
python3 DevUtils/PublishMattOSPackage.py publish
```

Each invocation downloads the latest `ManageMattOSRepository.py` into the ignored
`DevUtils/.downloaded/` directory and explicitly selects `--repo mattpackages`.
The manager owns server access, authentication, signing and repository updates;
Slate stores no separate publishing credentials. Failed downloads never run an
old cached manager. Publishing checks that the workspace version is newer than
all published Slate versions, builds the package, reads `builds/latest-build.env`
without evaluating shell code, validates the package name/version/architecture,
and uploads with overwrite protection. `--dry-run` builds and validates without
uploading; `--package PATH` selects an already built package and skips rebuilding.
Before the next release, bump `[workspace.package].version` in Cargo.toml, the
source of truth; the build updates Cargo.lock to match. Generated packages and
build metadata stay in the ignored `builds/` directory. Run `python3 -m unittest discover -s tests -v` for publisher tests.

Once the MattPackages APT repository is configured, install with `sudo apt install
slate`; subsequent releases arrive through normal APT updates.

## Working prototype features

- File-aware editor-only / workspace startup in both frontends; configurable defaults.
- Lazy directory browsing (including parent navigation); open UTF-8 files,
  create untitled buffers, save, and save as a new file.
- Grapheme-aware cursor movement, selection, mouse selection, copy/cut/paste,
  configurable line numbers, incremental undo/redo, and multiple documents.
- Unicode literal search, forward/backward wrapping, match-case and whole-word
  options, match highlighting, replace-next and undoable replace-all.
- Go-to-line, configurable tab width/spaces, automatic indentation and selected
  line indentation/outdent. Syntax highlighting is shared by both frontends and
  runs on a separate worker using Syntect's bundled language definitions.
- Shared configurable shortcuts, context-sensitive hints and cursor position.
  The GUI editor follows the desktop palette and uses adaptive tabs and measured controls;
  the TUI has explicit focus borders and interactive editing prompts.
- The GUI sends compact changed text rows directly to native Qt views, retaining
  shaped text layouts during cursor movement and selection. QML receives pane
  metadata; input and worker redraws are coalesced while Rust applies every edit
  immediately. Terminal updates send changed rows, and idle checks skip snapshots.
- Split views of the same document share text/history and keep independent
  cursors and scroll positions.
- Nested horizontal/vertical splits, resizing, selectable view groups,
  pane swapping/removal, and editor-only or bottom-terminal presets.
- Named layouts saved in `$XDG_CONFIG_HOME/slate/layouts.toml` (normally
  `~/.config/slate/layouts.toml`), usable from either frontend. Across launches,
  saved layouts restore shape and view kinds; editor views bind to the opened
  document and terminal views start new shells.
- Automatic workspace checkpoints restore open documents, unsaved buffers,
  split/tab arrangements, focus, cursor/scroll positions and browser directory.
  Terminal shells restart in their remembered directory.
- Real PTY shells with shared VT screen emulation, colors, alternate screens,
  cursor keys, bracketed paste, bounded scrollback, window-size propagation,
  application mouse reporting and frozen viewport selection/copy.
- Multiple terminal sessions remain alive when their view is hidden. Closing
  a pane leaves sessions running; closing a terminal tab or `terminate-terminal`
  explicitly stops that session.
- Git branch/status, staged/unstaged/untracked groups, direct stage/unstage,
  read-only diffs and untracked previews, bulk/section staging, and a commit
  message composer. Directory listing and Git commands have separate workers. File opens/saves have their own worker;
  saving a snapshot leaves edits made during the save dirty. Git operations use
  the repository root, including when opening a subfolder.
- Atomic saves preserve permissions, owner/group, extended attributes, hard
  links and symlink targets; external disk edits and unintended Save As
  overwrites are refused. Read-only files ask first; unwritable files offer
  sudo/pkexec. Quitting with unsaved documents asks to save, discard or cancel.
- Encodings (UTF-8/BOM, UTF-16, legacy 8-bit), LF/CRLF/CR line endings, binary
  file detection, new files from the command line, `+LINE`, standard input and
  read-only views.
- nano workflows: optional nano keymap and hint bar, cut-line chains, mark,
  justify, hard and soft wrap, spell checking, word count, cursor position
  report, insert file, help, suspend, backups and `NAME.save` emergency saves.
- Small windows show the focused pane; cycling focus accesses the other panes.

## Controls

| Key | Action |
| --- | --- |
| F1 or Ctrl+Shift+P | Command palette |
| F6 | Next pane; also escapes input captured by a terminal program |
| F7 | Next tab/view in the current pane |
| F8 | New terminal in the current pane |
| F9 / Shift+F9 | Split right / below |
| F10 | Expand / collapse workspace |
| Ctrl+Q | Quit (checks for unsaved changes) |
| Ctrl+, | Settings (when supported by the outer terminal) |
| Ctrl+S in an editor | Save |
| Ctrl+Z / Ctrl+Y | Undo / redo |
| Ctrl+F | Find prompt |
| Ctrl+H or Alt+R in an editor | Replace prompt |
| Ctrl+G | Go to line |
| F3 / Shift+F3 | Find next / previous |
| Tab / Shift+Tab; Ctrl+] / Ctrl+[ | Indent / outdent |
| Ctrl+Shift+C / Ctrl+Shift+V in a terminal | Copy selection / paste |
| Alt+C / Alt+V in a terminal | Copy/paste when the outer terminal cannot distinguish shifted control keys |
| Shift+arrows; mouse drag | Select text |
| Ctrl+A / C / X / V in an editor | Select all / copy / cut / paste |
| Ctrl+Left / Right, Ctrl+Backspace / Delete | Move by word, delete a word |
| Home | First non-blank character, then column 1 |
| Alt+Z | Toggle soft wrap |

The command palette searches readable action names and descriptions. Use Up/Down
and Enter to select an action; unavailable actions explain what is missing.
Actions needing a path, line, layout name, or commit message open an input form.
Both frontends use the same Rust catalog and command handlers. Prefix input with
`:` to execute an advanced textual command directly.

GUI: double-click a file to open it and drag dividers to resize. The menu bar
provides **File**, **Edit**, **View**, **Go** and **Run** menus (Run holds
debugging, breakpoints, tasks and terminals). Menu actions show configured
shortcuts and are disabled when unavailable in the focused pane. Right-clicking
the text offers go to definition, references, rename, code actions, format and
breakpoints. Find and replace open at the bottom of the window without dimming
the text, so matches stay visible; Alt+C and Alt+W toggle their options.
Settings lives under File; workspace visibility and saved layouts live under View.
Each pane has a **⋮** action menu for splits, view changes, and relevant document,
terminal, or Git actions. Right-click opens the pane menu in files and a file action menu in Git.
Tabs fit within their header and show complete names in tooltips. When space is
limited, the active tab stays visible and a dropdown lists every tab. Headers and file rows grow with the interface font. Pane minimum
sizes constrain displayed split ratios without changing saved preferences. When
the layout cannot fit, only the focused pane is displayed; F6 changes focus and
expanding the window restores the full arrangement. Dialog contents scroll when
the window is too short.

The Git pane shows the branch and separates staged, unstaged, untracked, and
conflicted files. Its +/− controls stage or unstage that file; double-click opens
the comparison for that group. Staged diffs compare the index to HEAD (including
an initial commit); unstaged diffs compare the working file to the index.
Untracked previews compare the file to an empty file. Git errors remain visible,
and Refresh reloads changes made outside Slate. **Stage all** and **Unstage all**
operate on the entire repository; section +/− controls operate on that group.
Unstaging preserves working files. The GUI shows section counts, file names and
parent folders, and reserves a gutter for the scrollbar beside the row actions.

Type a message in the GUI composer and click **Commit**, or press **Ctrl+Enter**.
Commit operates on staged files. Short panes open a compact message dialog.
The draft survives pane/layout changes and failed commits; successful commits
clear it. Hover file controls for explanations, or right-click a file for actions.
In the TUI Git pane, S stages, U unstages, C opens the commit form, R refreshes,
and Enter inspects the selected change.

Settings provides file/directory startup defaults, theme, indentation, spaces/tabs,
auto-indent and line-number controls in both frontends. GUI Settings also offers
an action to open the settings file for keybinding customization.

TUI: Enter or click opens the selected entry;
mouse dragging resizes dividers. In a TUI editing prompt, Tab switches find and
replacement fields, Enter finds/replaces next, Ctrl+Enter replaces all when the
outer terminal distinguishes that key, and Alt+C / Alt+W toggle case/whole-word.
`replace-all` also works through the palette. Escape closes the prompt. Legacy
terminals can encode Ctrl+H as Backspace; Alt+R opens replace reliably.

Terminal applications that request mouse input receive it. Hold Shift to select
text or scroll the terminal's history instead. Selection freezes the displayed
viewport while output continues; typing, pasting, resizing or scrolling returns
to the current terminal view. The GUI uses the system clipboard. TUI copy uses
OSC 52 when the outer terminal
supports it; pasting from the outer terminal works through bracketed paste.

Advanced commands (entered with a leading `:` in the palette) accept a verb followed by its argument. Paths and commit messages can
contain spaces without quoting. Useful examples:

```text
find some text
replace some text => replacement
replace-all some text => replacement
goto 42
indent
outdent
set indent-width 2
set insert-spaces true
set editor-theme dark
set terminal-theme dark
set file-startup editor-only
set directory-startup workspace
settings
settings-reload
editor-only
workspace
toggle-workspace
open /path/to/code.rs
save-as /path/to/new-file.rs
split-right
split-down
split-terminal-down
terminal
terminate-terminal
files
git
editor
close-pane
move-pane 3
resize 4 0.25
preset development
preset bottom_terminal
preset minimal
layout-save coding
layout-load coding
stage
unstage
diff
commit Implement the first feature
quit
```

`stage`, `unstage`, and `diff` use the selected Git entry.
`stage-all` and `unstage-all` operate on the entire repository. A diff opens as a
named read-only inspection view. Inspecting a diff does not mark the workspace dirty. `close` closes the focused file or terminal tab, whereas `close-pane`
removes its presentation. Explicit `discard-document` and `discard-quit`
commands discard unsaved work. GUI window closure asks before discarding.

## Settings and recovery

`set` writes `$XDG_CONFIG_HOME/slate/settings.toml` (normally
`~/.config/slate/settings.toml`). Supported options: `indent-width` (1–16),
`insert-spaces`, `auto-indent`, `line-numbers` (booleans), `editor-theme`
(`auto`, `dark`, `light`), `terminal-theme` (`auto`, `dark`, `light`, `editor`),
`theme` (an alias for `editor-theme`), `keymap` (`default`, `nano`; choosing one replaces the
key tables), `soft-wrap`, `wrap-column` (10–500), `hard-wrap`, `backup`,
`file-recovery`, `tui-mouse`, `terminal-clipboard`, `complete-while-typing`
and `accept-completion-on-enter`; `set` with an unknown name lists them all. Running as root through `sudo` without `-H`,
Slate uses root's own configuration and state directories rather than creating
root-owned files in your home directory. Run `set indent-width 4` to create a complete settings
file, edit its `global_keys`, `editor_keys`, `terminal_keys` and `debug_keys` tables, then run
`settings-reload`. Chords use the order `Ctrl+Alt+Shift+key` with lowercase key
names, such as `Ctrl+s`, `Shift+f3`, or `Alt+r`. A supplied key table replaces that
scope's defaults; removing an entry disables it. F1/Ctrl+Shift+P remain frontend
palette shortcuts; F6 remains a reserved route out of terminal input capture.

Directory workspaces always keep recovery checkpoints. Individually opened files
(and standard input) do not, unless `file_recovery = true`: then each file has
its own checkpoint that restores only that file. Checkpoints contain the text of
unsaved buffers only; clean files are recorded as paths and reread from disk, so
opening a secret never copies it into the state directory.

Each canonical workspace directory has a private, atomic checkpoint under
`$XDG_STATE_HOME/slate/workspaces/` (normally `~/.local/state/slate/workspaces/`).
Checkpoints run at most once a second when state changes, and are flushed on
normal exit. A forced crash can lose edits newer than the last completed
checkpoint; this is recovery, not file autosave. Checkpoint files contain buffer
contents and original save baselines and are mode 0600 on Unix. A per-workspace
lock prevents a second instance from overwriting the first instance's state.
The second instance can still edit, with recovery disabled and a status message.
Malformed/oversized checkpoints remain untouched and recovery is disabled for
that launch; `--fresh` explicitly starts a new workspace state.

Restoring clean files reads current disk content; restoring dirty files preserves
unsaved text and its original disk baseline, so external changes are still
rejected on save. Missing clean files are retained as recoverable dirty buffers.
Explicit discard-and-quit discards buffer changes in the checkpoint too. Undo
history is not restored. Checkpoints are limited to 128 MiB; failures are reported
without replacing the previous checkpoint. Startup restoration runs before the
UI opens; interactive file operations run on the worker.

Terminals restart fresh shells, rather than replaying commands or restoring
running processes. On Linux, the shell's directory is read through `/proc` when
permitted; OSC 7 shell integration is also supported. Without either source,
the terminal's launch directory is retained. A shell can report its directory
with `printf '\033]7;file://localhost%s\007' "$PWD"` in its prompt hook.

## Architecture and dependency policy

- `slate-core`: editing, documents/views, commands, layout tree, file/Git
  services, preferences/search/highlighting, workspace recovery, terminal processes,
  and VT emulator state. IDE services keep their own state in their own modules:
  the language-server client (`lsp`), debug adapter client (`dap`), tasks, project
  index/search, pickers and external tools. They run their processes and reader
  threads and report to the main loop as typed `ide::Event`s, so editor state is
  changed in one place.
- `slate-cli`: Ratatui/Crossterm presentation and terminal input.
- `slate-gui`: Qt Quick QML and a thin Qt C++ presentation adapter (editor and
  terminal surfaces, minimap, accessibility, printing). A small JSON/C ABI
  connects it to the Rust core; editing and PTY logic remain Rust.
- `slate` (root package) is the terminal executable and does not link Qt;
  `--gui` execs the `slate-gui` executable built by the `slate-gui` crate.

**No exact runtime dependency constraints.** Qt discovery specifies a minimum
public API, without CMake `EXACT`; QML imports have no numeric version pins.
Use compatible distro-provided Qt 6 updates. Cargo manifests use
compatible version ranges, never `=version` requirements. `Cargo.lock` records
build resolution for reproducibility; it does not require an exact installed
Qt runtime version. Major ABI/API changes may require rebuilding.

## Validation

```bash
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p slate --test tui_nano      # nano workflows in a real PTY
cargo test -p slate --test tui_ide       # pickers, carets, folds, language server in a PTY
cargo test -p slate-core --test lsp      # LSP client against tests/fixtures/fake_lsp.py
cargo test -p slate-core --test debug    # gdb -i dap on a compiled C program
python3 scripts/tui-smoke.py target/debug/slate
python3 scripts/tab-close-smoke.py target/debug/slate
python3 scripts/recovery-smoke.py target/debug/slate
python3 scripts/install-smoke.py target/release/slate
python3 scripts/dependency-policy.py
cargo build -p slate -p slate-gui --features slate-gui/smoke
python3 scripts/gui-offscreen-smoke.py target/debug/slate
python3 scripts/gui-layout-smoke.py target/debug/slate  # Qt Test, Basic/Fusion/KDE styles
SLATE_GUI_DESKTOP_SMOKE=1 python3 scripts/gui-offscreen-smoke.py target/debug/slate
SLATE_GUI_IDE_SMOKE=1 python3 scripts/gui-offscreen-smoke.py target/debug/slate
python3 scripts/single-instance-smoke.py target/debug/slate-gui
SLATE_GUI_FEATURE_SMOKE=1 python3 scripts/gui-offscreen-smoke.py target/debug/slate
SLATE_GUI_FILE_DIALOG_SMOKE=1 python3 scripts/gui-offscreen-smoke.py target/debug/slate
SLATE_GUI_TAB_CLOSE_SMOKE=1 python3 scripts/gui-offscreen-smoke.py target/debug/slate
QT_QPA_PLATFORMTHEME=kde SLATE_GUI_PLATFORM=wayland SLATE_GUI_REQUIRE_KDE_DIALOGS=1 SLATE_GUI_FILE_DIALOG_SMOKE=1 python3 scripts/gui-offscreen-smoke.py target/debug/slate # running Plasma desktop
SLATE_GUI_GIT_SMOKE=1 python3 scripts/gui-offscreen-smoke.py target/debug/slate
SLATE_GUI_COMMAND_SMOKE=1 python3 scripts/gui-offscreen-smoke.py target/debug/slate
cargo build --release -p slate -p slate-gui --features slate-gui/smoke
python3 scripts/gui-performance-smoke.py target/release/slate
python3 scripts/idle-smoke.py target/release/slate       # Linux, release GUI/TUI idle CPU
python3 scripts/gui-smoke.py target/debug/slate   # optional X11: Xvfb + xdotool
SLATE_GUI_PLATFORM=wayland SLATE_GUI_FEATURE_SMOKE=1 python3 scripts/gui-offscreen-smoke.py  # running Wayland desktop
```

Core tests cover Unicode editing/search/replacement, shared views, incremental
history, save conflicts/permissions/symlinks, asynchronous saves during editing,
recovery/disk baselines, layout geometry, syntax colors, PTY mouse/paste/selection,
terminal directories and unsaved-work protection. Optional Vim/htop/SSH/tmux
integration checks run when tools are available; `SLATE_REQUIRE_TERMINAL_TOOLS=1`
makes SSH/tmux availability and execution mandatory. CI installs those tools,
checks the Qt 6.4 adapter API, and runs GUI workflows with distro-provided Qt
(without KDE frameworks) in Fedora and Debian containers. Container jobs exercise their current
compatible packages without exact runtime version constraints.

Smoke tests drive actual GUI/TUI input, saves, editing prompts, shell execution,
clipboard interactions, named layouts, installed entry points and forced-crash
recovery. GUI layout checks cover Basic and Fusion, plus a duplicate-caption
regression check for KDE's desktop style when that QML style is installed.
The populated Git workflow checks normal/narrow/short panes and large fonts,
scrollbar/action separation, bulk and section staging, and commit hook failures.
Set `SLATE_GUI_ARTIFACT_DIR` to retain its screenshots and report.
Release performance checks measure actual input handlers and rendered frames at
normal, 1080p and 4K sizes, plus cursor/selection layout retention, scrolling,
syntax highlighting, batched input and typing beside a busy terminal. Set `SLATE_GUI_PERF_REPORT` to
save the measurements, or `SLATE_GUI_PLATFORM=wayland` to use the desktop renderer.
For a Qt-free installation, use
`./scripts/install.sh "$HOME/.local" --tui-only`. The GUI installation also installs
a desktop launcher and scalable icon; put the selected prefix's `bin` in PATH.

Documents are ropes (up to 1 GiB). Undo stores inserted and removed spans,
with a 32 MiB payload budget and up to 10,000 edits, never whole-buffer
copies. Several edits from one action (carets, rename, formatting) undo as one
step. Syntax highlighting is incremental: it is checkpointed every 32 lines,
resumes from the line an edit touched, and does the visible rows first.
Terminals support conventional VT behavior, not Sixel/Kitty graphics or every
xterm extension. Agent integration remains future work in [GOALS.md](GOALS.md).

See [DESIGN.md](DESIGN.md) for the implementation decisions and validation scope.

Build validation note: the standard release build passed with Rust 1.94. Rust
1.99 produced undefined-symbol linker errors with thin LTO in the validation
environment; its release build passed with `CARGO_PROFILE_RELEASE_LTO=false`.
This does not change Slate's compiler or runtime dependency requirements.
