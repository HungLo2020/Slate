# Slate

A Rust workspace editor with a Kirigami GUI and a terminal UI sharing the same
editor core. This repository contains a Linux prototype with recoverable workspaces.

## Build and launch

Install Rust **1.89 or newer**, a C++17 compiler, CMake, pkg-config, and Qt 6
Core/Gui/Widgets/Qml/Quick development packages. GUI runtime packages must include
**Kirigami 6** and Qt Quick Controls, Layouts, Templates, Window, QtQml, Models,
and WorkerScript modules. Git is used for the Git pane. Your shell is taken
from `SHELL`, falling back to `/bin/sh`.

On a current KDE/Kubuntu installation, install the distro's Qt 6 development
packages and Kirigami 6 QML package. Package names vary with distro release;
older distributions may only package Kirigami for Qt 5, which is insufficient.
Slate does not download or vendor Qt or Kirigami.

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
./target/release/slate --gui .            # Kirigami mode (runs slate-gui)
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
keymap = "default"        # or "nano"
soft_wrap = false
wrap_column = 80          # justify and hard wrap
hard_wrap = false
backup = false            # keep NAME~ on save
file_recovery = false     # recover unsaved single files after a crash
tui_mouse = true
```

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
Before the next release, bump `[workspace.package].version` and refresh Cargo.lock
with Cargo. Generated packages and build metadata stay in the ignored `builds/`
directory. Run `python3 -m unittest discover -s tests -v` for publisher tests.

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
provides **File**, **Edit**, **View**, **Go**, and **Terminal** menus. Menu actions
show configured shortcuts and are disabled when unavailable in the focused pane.
Settings lives under File; workspace visibility and saved layouts live under View.
Each pane has a **⋮** action menu for splits, view changes, and relevant document,
terminal, or Git actions. Right-click opens the pane menu in files/editors and a file action menu in Git.
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
set theme auto
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
`insert-spaces`, `auto-indent`, `line-numbers` (booleans), `theme`
(`auto`, `dark`, `light`), `keymap` (`default`, `nano`; choosing one replaces the
key tables), `soft-wrap`, `wrap-column` (10–500), `hard-wrap`, `backup`,
`file-recovery` and `tui-mouse`. Running as root through `sudo` without `-H`,
Slate uses root's own configuration and state directories rather than creating
root-owned files in your home directory. Run `set indent-width 4` to create a complete settings
file, edit its `global_keys`, `editor_keys` and `terminal_keys` tables, then run
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
  and VT emulator state.
- `slate-cli`: Ratatui/Crossterm presentation and terminal input.
- `slate-gui`: Kirigami QML and a thin Qt C++ presentation adapter. A small
  JSON/C ABI connects it to the Rust core; editing and PTY logic remain Rust.
- `slate` (root package) is the terminal executable and does not link Qt;
  `--gui` execs the `slate-gui` executable built by the `slate-gui` crate.

**No exact runtime dependency constraints.** Qt discovery specifies a minimum
public API, without CMake `EXACT`; QML imports have no numeric version pins.
Use compatible distro-provided Qt 6/Kirigami 6 updates. Cargo manifests use
compatible version ranges, never `=version` requirements. `Cargo.lock` records
build resolution for reproducibility; it does not require an exact installed
Qt/Kirigami runtime version. Major ABI/API changes may require rebuilding.

## Validation

```bash
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p slate --test tui_nano      # nano workflows in a real PTY
python3 scripts/tui-smoke.py target/debug/slate
python3 scripts/tab-close-smoke.py target/debug/slate
python3 scripts/recovery-smoke.py target/debug/slate
python3 scripts/install-smoke.py target/release/slate
python3 scripts/dependency-policy.py
cargo build -p slate -p slate-gui --features slate-gui/smoke
python3 scripts/gui-offscreen-smoke.py target/debug/slate
python3 scripts/gui-layout-smoke.py target/debug/slate  # Qt Test + Kirigami 6
SLATE_GUI_FEATURE_SMOKE=1 python3 scripts/gui-offscreen-smoke.py target/debug/slate
SLATE_GUI_FILE_DIALOG_SMOKE=1 python3 scripts/gui-offscreen-smoke.py target/debug/slate
SLATE_GUI_TAB_CLOSE_SMOKE=1 python3 scripts/gui-offscreen-smoke.py target/debug/slate
QT_QPA_PLATFORMTHEME=kde SLATE_GUI_PLATFORM=wayland SLATE_GUI_REQUIRE_KDE_DIALOGS=1 SLATE_GUI_FILE_DIALOG_SMOKE=1 python3 scripts/gui-offscreen-smoke.py target/debug/slate # running Plasma desktop
SLATE_GUI_GIT_SMOKE=1 python3 scripts/gui-offscreen-smoke.py target/debug/slate
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
checks the Qt 6.4 adapter API, and runs GUI workflows with distro-provided Qt and
Kirigami in Fedora and Debian containers. Container jobs exercise their current
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

Buffers remain UTF-8 strings with a 16 MiB per-buffer limit. Undo stores inserted
and removed spans with a 32 MiB payload budget and up to 10,000 edits, rather than
whole-buffer copies; full-buffer replacement still needs a full edit payload.
Syntax parsing is background work and caches generation-tagged results; it is not
an incremental parser. Terminals support conventional VT behavior, not
Sixel/Kitty graphics or every xterm extension. LSP, debugging and agent integration
remain future work in [GOALS.md](GOALS.md).

See [DESIGN.md](DESIGN.md) for the implementation decisions and validation scope.

Build validation note: the standard release build passed with Rust 1.94. Rust
1.99 produced undefined-symbol linker errors with thin LTO in the validation
environment; its release build passed with `CARGO_PROFILE_RELEASE_LTO=false`.
This does not change Slate's compiler or runtime dependency requirements.
