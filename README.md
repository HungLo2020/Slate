# Slate

A Rust workspace editor with a Kirigami GUI and a terminal UI sharing the same
editor core. This repository contains an initial Linux prototype.

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
  line numbers, undo/redo, and multiple documents.
- Split views of the same document share text/history and keep independent
  cursors and scroll positions.
- Nested horizontal/vertical splits, resizing, selectable view groups,
  pane swapping/removal, and editor-only or bottom-terminal presets.
- Named layouts saved in `$XDG_CONFIG_HOME/slate/layouts.toml` (normally
  `~/.config/slate/layouts.toml`), usable from either frontend. Across launches,
  saved layouts restore shape and view kinds; editor views bind to the opened
  document and terminal views start new shells.
- Real PTY shells with shared VT screen emulation, colors, alternate screens,
  cursor keys, bracketed paste, bounded scrollback, and window-size propagation.
- Multiple terminal sessions remain alive when their view is hidden. Closing
  a pane leaves sessions running; `terminate-terminal` explicitly stops one.
- Git status, stage/unstage, diff, and commit actions. Directory listing and Git
  commands run on a background worker. Git operations use the workspace root.
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
| Shift+arrows; mouse drag | Select text |
| Ctrl+A / C / X / V in an editor | Select all / copy / cut / paste |

GUI: double-click a file to open it, drag dividers to resize, and right-click a
pane for view and Git actions. TUI: Enter or click opens the selected entry;
mouse dragging resizes dividers. TUI copy uses OSC 52 when the outer terminal
supports it; pasting from the outer terminal works through bracketed paste.

Commands accept a verb followed by its argument. Paths and commit messages can
contain spaces without quoting. Useful examples:

```text
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

## Architecture and dependency policy

- `slate-core`: editing, documents/views, commands, layout tree, file/Git
  services, terminal processes, and VT emulator state.
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
cargo build --features gui-smoke
python3 scripts/gui-offscreen-smoke.py target/debug/slate  # Qt Test + Kirigami 6
python3 scripts/gui-smoke.py target/debug/slate   # optional X11: Xvfb + xdotool
```

Core tests cover Unicode editing, shared split documents, undo/redo, save
conflicts/permissions/symlinks, layout geometry, PTY colors/resizing/alternate
screens, and unsaved-work protection. Smoke tests exercise actual frontend
input, file writes, shell execution, and layout persistence.

This is a prototype: buffers are UTF-8, limited to 16 MiB, backed by strings,
and keep bounded snapshot-based undo history. File open/save runs synchronously;
large-file optimization and fully asynchronous document I/O remain follow-up
work. Terminals implement conventional VT behavior, not Sixel/Kitty graphics or
all xterm extensions. Keybindings are currently fixed. Syntax highlighting,
LSP, debugging, and agent integration remain future features in [GOALS.md](GOALS.md).
