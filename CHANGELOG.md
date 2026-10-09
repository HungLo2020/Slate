# Changelog

## Unreleased

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
