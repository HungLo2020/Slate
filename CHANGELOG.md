# Changelog

## 0.1.17 — 2026-10-22

- Add a saved **Preferred GUI diff viewer** setting for unified or side-by-side Git comparisons.
- Link vertical, horizontal and smooth scrolling between comparison panes; close both halves together and restore the previous layout while preserving other files opened in those panes.
- Preview PNG, JPEG, SVG and other supported images in the GUI, with zoom, fit, panning, live reload and an editable SVG source view.
- Keep horizontal scrollbars available when narrow panes or settings hide the minimap.
- Cover image previews, paired diff scrolling, comparison cleanup and session recovery with core and GUI regression tests.

## 0.1.16 — 2026-10-20

- Put a terminal program's copy (OSC 52) on the desktop clipboard immediately, again only with **Terminal programs may set the clipboard** allowed, instead of leaving it until the next explicit copy. The clipboard event is consumed once, so it is not repeated in later frames or restored from a saved one.
- Refuse Copy and Cut in a GUI terminal with no selection, so the shortcut and the pane menu no longer replace the desktop clipboard with stale text and the reason explains that Shift+drag selects while a program owns the mouse.
- Check the GUI's terminal clipboard end to end from a separate Qt client, including the pane menu, an alternate screen and a program's own copy (`scripts/gui-clipboard-smoke.py`).

## 0.1.15 — 2026-10-11

- Keep every TUI tab reachable in narrow panes: crowded tab bars number their tabs and shorten titles (the active tab's longest), then scroll a window of tabs around the active one with clickable ‹ and › instead of hiding all but the active tab.
- Switch to the previous tab with Shift+F7, and switch tabs from a focused terminal with Ctrl+PageDown / Ctrl+PageUp; the terminal hint bar names the tab keys when the pane has more than one tab.

## 0.1.14 — 2026-10-10

- Render GUI editor panes again only when their content changes, not on every terminal, Git or worker update; and build file and Git views only in panes that show them.
- Make screen cells compact (colours as numbers, glyphs inline), shrinking every terminal and editor screen and removing three allocations per cell.
- Match brackets by walking the text once instead of looking up each character; repaint only changed terminal rows, drawing them as cached glyph runs.
- Send settings, recent lists, profiles and colours to the GUI only when they change, and stop re-querying the input method on every frame.
- Keep one copy of file, Git and history lists from the core to Qt (shared in Rust, serialized directly, compact rows in Qt), and stop resending an unchanged Git status.
- Merge directory listings requested while one is being read (several per save before), and retry one the busy queue refused instead of dropping it.
- Bound memory: a scrollback budget shared by all terminals, highlight spans kept only near what is shown, parser state dropped for closed documents, and selections that freeze only the visible terminal rows.

## 0.1.13 — 2026-10-10

- Make the GUI menu bar and status bar fit one line of text instead of reserving full-size button height.
- Size the Git commit message box to its text: one line when empty, growing to six lines before scrolling.
- Name the system fixed-width font in the Settings font list, which showed it as a blank entry.

## 0.1.12 — 2026-10-10

- Open folders consistently: Open Folder, Recent Projects, the palette and dropped folders all ask whether to use this window or a new one (or follow the new "Open folders in" setting). The current folder is shown instead of reopened, a folder open in another window is never opened twice without recovery, and new windows open offset from the one that started them.
- Choosing a startup mode in Settings now takes effect while the minimal or workspace profile is active: it switches to the default profile, keeping the profile's other startup choice, instead of silently snapping back.

## 0.1.11 — 2026-10-10

- Theme the file browser and Git pane separately ("File browser theme", "Git pane theme"). Both use the editor's colours by default instead of the desktop's; they can follow the desktop (Qt) palette or use dark or light. Pane tabs, menus, hints and dialogs keep the desktop palette. Terminals now also use the editor's colours by default; their own dark palette remains a choice.
- Add a commit graph below the Git pane's changes in both interfaces: HEAD and its upstream with coloured lanes, branch and tag badges, authors and ages, paged loading, commits opened read-only, and a draggable divider to resize the graph against the changes.
- Redesign the GUI Git pane: one line per change with file icons and coloured status letters, hover row actions, collapsible groups with count badges, focus-aware selection, a non-shifting progress bar, a primary Commit button that stays recognisable when unavailable, and consistent menus.
- Show the branch's upstream with ahead/behind counts; add fetch, fast-forward pull, push (setting an upstream on first push), branch switching and branch creation. Remote commands never wait on a password prompt.
- Add "Discard changes" (with confirmation; untracked files go to Trash), "Open changed file" and "Reveal in file browser" to both interfaces.
- Lighten pane tabs: the active tab is tinted and underlined rather than filled, and tool tabs fit side by side in narrow panes.
- Fix the Git pane highlighting a different row than the selected one after rows were inserted or removed.

## 0.1.10 — 2026-10-10

- Redesign the GUI file tree: theme file-type icons, indent guides, denser rows, focus-aware selection, Git status colors and folder markers, dimmed ignored entries, a folder header with up, new, collapse-all and refresh actions, an "Up to …" parent row, a useful empty state and path hints only for truncated names.
- Expand and collapse GUI folders without rebuilding the list or losing its scroll position.
- Add "Collapse all folders" and "Open terminal here" actions to both interfaces, and copy-path, relative-path and containing-folder entries to the GUI file menu.

## 0.1.9 — 2026-10-10

- Fix crashes from rectangular selection over lines ending in non-ASCII text and from a zero `tab_width` in project settings.
- Ask before discarding from the command palette, typed commands and key bindings in every interface; keep unsaved, piped and waited-on buffers when layouts change or limits are reached.
- Hand the terminal to sudo during elevated saves so Ctrl+C or Ctrl+Z cancels the save rather than Slate.
- Keep the single-instance socket in a private, owner-checked directory and verify peer credentials on both ends.
- Remember trust only for workspaces and repository roots, never for broad folders such as home or temporary directories; store trusted paths exactly, lock updates, and fail closed on unreadable trust files.
- Read project configuration, changed files and save baselines with size limits and without blocking on special files.
- Save settings by editing only the changed keys, preserving comments, other instances' edits and session-only options.
- Clear inherited Git repository variables and relative `PATH` entries for helpers, servers, tasks and terminals; resolve helpers only from absolute, executable `PATH` entries.
- Undo multi-cursor and large operations as whole steps, bound undo memory, normalize formatter and language-server line endings, and keep replace-in-files backups per run with atomic, retried restores.
- Bound task output, report task exit when the task's shell exits, signal stopped tasks once, and restart crashed language servers with backoff while coalescing full-document changes for busy servers.
- Recover from interrupted GUI requests with stable `.save` copies; handle termination before the GUI starts, worker-thread panics in the terminal interface, and plain-text rendering of untrusted names.
- Reduce per-keystroke and per-frame work: incremental highlighting, soft wrap and caret rebasing, cached recovery blobs, compact GUI terminal rows, visible-only TUI lists, and event-driven input and timers.
- Expire unused recovery stores, damaged checkpoints, temporary files and file-use locks safely; verify the repository manager by pinned commit and SHA-256 before publication.

## 0.1.8 — 2026-10-09

- Add `--wait` for GUI editor integration, with per-document completion in new or existing windows and failure reporting on open errors or termination.
- Bound document search, replacement and occurrence selection; perform large searches, word counts and save preparation on workers, and refuse stale results.
- Add regex capture replacement, selection scope, rectangular selection, search history, and explicit project-search scope controls and skipped-file counts.
- Apply EditorConfig indentation, tab width and save formatting. Support exact Latin-1 conversion.
- Open directories as workspaces, retain recent projects, persist GUI geometry, edit shortcuts graphically, and provide a bounded support report.
- Add aligned Git comparisons, guarded hunk staging and undoable conflict choices.
- Add LSP signature help, initialization options, server configuration and validated transactional file operations.
- Add nano-style launch options and optional file-use warnings.
- Pin the release toolchain, repair rooted-browser GUI smoke tests, and require successful checks on committed, pushed main before publication.

## 0.1.7

- Keep file navigation inside the opened workspace in both interfaces, including symlink targets.
- Bound queued PTY input and report overload without blocking the interface.
- Store large recovery buffers separately, expose checkpoint failures, retry, salvage unaffected documents, and drain saves during shutdown.
- Extract session, save and service-event orchestration and correct staging/LSP/package documentation.
