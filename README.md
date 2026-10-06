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

```bash
cargo build --release
./target/release/slate .                  # terminal mode
./target/release/slate --gui .            # Kirigami mode
./target/release/slate src/main.rs        # open a file
```

Install the one executable and its GUI alias:

```bash
./scripts/install.sh "$HOME/.local"
slate .
slate-gui .
```

`--tui` and `--gui` override invocation-name selection. The default executable
contains both frontends. Terminal mode does not require a graphical session,
but a combined build still links Qt libraries. For machines without Qt, an
explicit TUI-only build is also available with `cargo build --no-default-features`.

## Working prototype features

- Default Files / Editor / Terminal layout in both frontends.
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
  The GUI editor follows the desktop palette and uses scrollable tabs/toolbars;
  the TUI has explicit focus borders and interactive editing prompts.
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
  a pane leaves sessions running; `terminate-terminal` explicitly stops one.
- Git status, stage/unstage, diff, and commit actions. Directory listing and Git
  commands run on a background worker. File opens/saves have their own worker;
  saving a snapshot leaves edits made during the save dirty. Git operations use
  the workspace root.
- Atomic saves preserve ordinary file permissions and symlink targets; external
  disk edits and unintended Save As overwrites are refused. Unsaved documents
  block normal close/quit.
- Small windows show the focused pane; cycling focus accesses the other panes.

## Controls

| Key | Action |
| --- | --- |
| F1 or Ctrl+Shift+P | Command palette |
| F6 | Next pane; also escapes input captured by a terminal program |
| F7 | Next tab/view in the current pane |
| F8 | New terminal in the current pane |
| F9 / Shift+F9 | Split right / below |
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

GUI: double-click a file to open it and drag dividers to resize. The top **Menu**
contains File, Edit, and Workspace commands; Open and Save appear as space allows.
Each pane has a **⋮** action menu for splits, view changes, and relevant document,
terminal, or Git actions. Right-click also opens the pane menu outside terminals.
Tabs scroll horizontally, keep the active tab visible, and show complete names
in tooltips. Headers and file rows grow with the interface font. Pane minimum
sizes constrain displayed split ratios without changing saved preferences. When
the layout cannot fit, only the focused pane is displayed; F6 changes focus and
expanding the window restores the full arrangement. Dialog contents scroll when
the window is too short.

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

Commands accept a verb followed by its argument. Paths and commit messages can
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
settings-reload
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

`stage`, `unstage`, and `diff` use the selected Git entry. A diff opens as an
untitled buffer. `close` closes the focused document, whereas `close-pane`
removes its presentation. Explicit `discard-document` and `discard-quit`
commands discard unsaved work. GUI window closure asks before discarding.

## Settings and recovery

`set` writes `$XDG_CONFIG_HOME/slate/settings.toml` (normally
`~/.config/slate/settings.toml`). Supported options: `indent-width` (1–16),
`insert-spaces`, `auto-indent`, `line-numbers` (booleans), and `theme`
(`auto`, `dark`, `light`). Run `set indent-width 4` to create a complete settings
file, edit its `global_keys`, `editor_keys` and `terminal_keys` tables, then run
`settings-reload`. Chords use the order `Ctrl+Alt+Shift+key` with lowercase key
names, such as `Ctrl+s`, `Shift+f3`, or `Alt+r`. A supplied key table replaces that
scope's defaults; removing an entry disables it. F1/Ctrl+Shift+P remain frontend
palette shortcuts; F6 remains a reserved route out of terminal input capture.

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
- The root binary dispatches between frontends; `slate-gui` is a symlink.

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
python3 scripts/tui-smoke.py target/debug/slate
python3 scripts/recovery-smoke.py target/debug/slate
python3 scripts/install-smoke.py target/release/slate
python3 scripts/dependency-policy.py
cargo build --features gui-smoke
python3 scripts/gui-offscreen-smoke.py target/debug/slate
python3 scripts/gui-layout-smoke.py target/debug/slate  # Qt Test + Kirigami 6
python3 scripts/gui-smoke.py target/debug/slate   # optional X11: Xvfb + xdotool
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
recovery. For a Qt-free installation, use
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
