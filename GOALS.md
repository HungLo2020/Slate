# Slate — Project Goals

Slate is a native Rust code editor with a Qt Quick graphical frontend and a
terminal frontend. Both are views of the same editor core and offer the same
configurable workspace model.

This document records the agreed direction before implementation. It describes
intended behavior, not features already implemented.

## Critical dependency rule

Runtime dependencies, including Qt, must never require an exact
version. Use compatible version ranges or minimum API requirements, versionless
QML imports, and distro-provided runtime packages.

## Two executables, shared core

- `slate` opens the terminal user interface (TUI). It must not link Qt, so it
  runs on servers, over SSH and on minimal systems.
- `slate-gui` opens the Qt Quick graphical interface (GUI) and can also run the
  TUI with `--tui`.
- `slate --gui` hands over to `slate-gui`; `slate-gui --tui` runs the TUI.
- Accept files or a workspace directory, such as `slate main.rs` or `slate .`,
  plus nano-style `+LINE` positions and `-` for standard input.
- Packaging keeps Qt optional: the terminal executable's libraries are hard
  dependencies and the graphical components are recommended.

## Crate structure and responsibilities

Start with a Cargo workspace containing these three principal crates:

| Crate | Responsibility |
| --- | --- |
| `slate-core` | Documents, editing, undo/redo, editor views, workspace state, commands, layout, settings, file operations, Git services, and terminal sessions. |
| `slate-cli` | TUI rendering, terminal input, focus interaction, shortcuts, and terminal-specific clipboard integration. |
| `slate-gui` | Qt Quick presentation, graphical input, and adapters exposing core state to QML. |

A thin binary entry point selects the frontend and composes these crates into
one program. Additional crates may be extracted when clear boundaries emerge.

**Shared editor behavior belongs in the core; each frontend owns its
presentation and input adaptation.** The core must not depend on Qt,
QML, or a TUI rendering framework. Frontends must not independently implement
editing rules, document history, Git operations, or terminal process ownership.

## Default workspace

Opening a file starts with only the editor. Opening a directory (or no path)
starts with three panes arranged side by side. Both defaults are configurable,
and `--editor-only` / `--workspace` explicitly override them:

| Position | Default content | Additional capabilities |
| --- | --- | --- |
| Left | File browser | Switch to Git and, later, search, symbols, or diagnostics. |
| Center | Text editor | Multiple files, editor views, and optional tabs. |
| Right | Terminal | Multiple terminal sessions, selectable or split into separate panes. |

This is a default layout, not a fixed application structure. Pane proportions
are configurable; no particular percentage is mandatory.

## Configurable layout model

- Represent layouts in the core as a tree of splits and pane groups.
- Splits arrange children side by side or stacked and store relative sizes.
- Pane groups contain selectable views; frontends may present these as tabs
  or another suitable control.
- Support resizing, adding, removing, splitting, moving, and focusing panes.
- Allow editor and tool views in different positions, including an editor-only
  workspace or terminals below the editor.
- Both frontends interpret the same logical layout and saved configuration.
  GUI dimensions use pixels; TUI dimensions use terminal cells.
- At small sizes, preserve usable minimum dimensions with deliberate collapse
  or focus behavior. Identical behavior does not require identical appearance.
- Save layout preferences and support named presets. The configuration format
  and default keybindings remain implementation decisions.

## Documents, views, and commands

A document owns its text, file identity, dirty state, and undo/redo history.
An editor view references a document and owns its cursor, selection, and
scroll position. Multiple views can display the same document without copying
its contents or creating separate histories.

Open documents, view groups, and their active selections belong to the shared
model. Visible tab bars are a presentation choice and can be disabled.

Expose shared user actions through a command system: open/save files, edit,
undo/redo, split panes, change focus, create terminals, and perform Git actions.
GUI controls and TUI shortcuts invoke the same commands. This also provides
the foundation for configurable bindings and a command palette.

## Real terminal panes in both frontends

- Each terminal session owns a shell or child process attached to a
  pseudo-terminal (PTY), terminal-emulator state, and bounded scrollback.
- Parse terminal control sequences and render the resulting screen; capturing
  stdout alone is insufficient.
- Support interactive programs, colors, cursor movement, alternate screens,
  resize events, and shell input.
- Translate frontend input into terminal input according to terminal modes.
  Define an accessible way to return focus to Slate when a child application
  consumes keys or mouse events.
- Terminal sessions are resources separate from layout panes. Switching a view
  must not restart its shell; closing a view and terminating a session are
  distinct actions.
- Multiple sessions must work in both frontends. Verify compatibility with
  representative programs such as shells, SSH, htop, Vim, and tmux rather than
  assuming every terminal extension is supported.
- Keep PTY and emulator behavior shared, while each frontend renders the
  terminal screen through its own presentation layer.

## Implementation direction and scope

Use Rust for editor and application logic, with Qt Quick (no KDE frameworks) for the
GUI. CXX-Qt is a candidate bridge, not a committed dependency. Prefer mature
text-buffer, parsing, PTY, and terminal-emulation libraries where appropriate;
specific choices require evaluation.

The first useful milestone is the same basic workflow in both frontends:
open a workspace, browse files, edit and save, undo/redo, resize and focus the
three panes, and run an interactive shell. Add configurable layouts, multiple
editor views, multiple terminals, and a shared Git viewer incrementally.

Language servers, incremental syntax highlighting, diagnostics, formatting,
debugging (DAP), build tasks with problem matching, project search, and
external tools are in place. Each is a service with its own state that reports
to the main loop as typed events, and none of them, including Git, runs until
the user trusts the folder it would run in. External agent integration is still future work; any agent
harness remains a separate system that Slate can integrate with.

## Quality goals

- Protect user work: handle save failures, external file changes, and unsaved
  document closure explicitly.
- Keep filesystem, Git, parsing, and process work from blocking frontend input.
- Favor responsive editing, low idle resource use, and efficient large-workspace
  navigation. Establish measured targets rather than promise an arbitrary
  binary size or performance figure.
- Verify core editing and layout behavior independently of either frontend.
- Exercise equivalent editing workflows in GUI and TUI, and validate real PTY
  interaction, resizing, and process cleanup.
- Keep shared features from drifting into two separate editor implementations.
