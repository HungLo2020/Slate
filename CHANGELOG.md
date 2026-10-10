# Changelog

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
