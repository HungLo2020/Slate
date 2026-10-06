# Daily editing milestone

Both frontends use the same commands, preferences, documents, views, layouts,
search rules and terminal resources. GUI controls and TUI prompts expose that
shared behavior. The native adapter remains a presenter; Rust owns editing,
recovery and process control.

## Editing and presentation

Literal search uses escaped Unicode regexes for optional case folding and word
boundaries. Replacements use literal replacement text: `$1` is text rather than
an unexpected capture expansion. Find wraps in either direction and records its
selection in the focused editor view. Replace-all is one reversible edit.
Indentation preserves existing line endings and lets the user choose tab stops,
spaces, automatic indentation and line numbers. Edits and undo/redo rebase other
views of the document while preserving their independent cursors/selections.

Syntect provides bundled language grammars and light/dark syntax palettes.
Parsing runs separately from file I/O and Git. Requests/results carry document
IDs, generations and theme choice; obsolete results are discarded. Highlighting
is scheduled only for visible documents and cached between frames. The parser
processes each document from its start, preserving multiline syntax state.

The GUI editor's base/text/selection colors follow Qt's desktop palette by
default. Syntax colors choose a light or dark palette from its luminance.
Terminal ANSI colors remain consistent with the emulator. GUI tabs and the toolbar
scroll when their contents exceed the available width. Focus borders, active tab
weight, cursor location and context-sensitive shortcut hints make state explicit.
The TUI presents equivalent prompts with keyboard controls and the same shared
rendered cells. Settings can force light/dark colors or remap scoped shortcuts.

## History and file I/O

Undo entries store a start offset, removed text, inserted text and cursor
positions. Ordinary typing no longer copies the entire document into history.
The regression test applies 100 three-byte edits to an 8 MiB buffer and retains
300 bytes of text payload, plus entry metadata. Strings remain the buffer storage;
this change does not claim rope-like insertion costs for very large files.

File open/save jobs have their own queue, independent of Git and highlighting.
Saving captures the buffer and original disk baseline. Completion updates the
save baseline/path without replacing the current buffer; edits made after the
request stay dirty. Concurrent saves of one document and closure during a save
are rejected. Duplicate opens reuse a document without losing unsaved changes.
Saves preserve permissions and symlink targets, recheck external changes and use
atomic replacement/no-clobber persistence. Files and containing directories are
synced on Unix. These checks detect ordinary external edits; the filesystem does
not provide a universal compare-and-swap against every concurrent external writer.

## Workspace recovery

Each canonical root owns a checkpoint and advisory lock. One instance writes
recovery for that root. JSON contains document text and save baselines, independent
views, layout/tab/focus state, browser directory and terminal working directories.
It is written through a private temporary file, synced and atomically replaced.
The UI queues changed state at most once a second; shutdown flushes pending I/O
and the final checkpoint before releasing the lock. Recovery does not save user
files or replay shell commands.

Restore validates schema, size, layout, resource references, ID bounds and UTF-8
cursor offsets before replacing live state. Clean buffers follow current disk
content; dirty buffers keep their original baseline so saving still detects a
conflict. Missing files remain recoverable. Discard-and-quit clamps views to the
discarded text so the next checkpoint remains valid. Corrupt checkpoints are
preserved and disabled for the launch; `--fresh` explicitly replaces that state.

## Terminal interactions

Portable PTYs run real shells; vt100 maintains the screen and scrollback. Input
uses the application's cursor mode, modifiers and bracketed paste settings.
Mouse presses/releases, button motion, all-motion and wheel events use the
application's requested legacy, UTF-8 or SGR encoding. Shift bypasses application
mouse capture for local selection and scrollback. Selection freezes a cloned VT
viewport while output continues, providing stable copying of wrapped/wide text.
Typing, paste, scroll and resize release the frozen selection.

GUI copy/paste uses the system clipboard; TUI copy uses OSC 52 and paste accepts
outer-terminal bracketed paste. Separate terminal-scoped shortcuts preserve
Ctrl+C as interrupt and Ctrl+V as terminal input. Shell directories use Linux
process inspection when available, with OSC 7 and the launch directory as
fallbacks. Restored terminals are new shells.

## Validation and compatibility

Regression tests cover editing/search/indentation, shared view state, history
payload bounds, save conflicts and background-save races, checkpoint round trips,
current disk content, syntax colors, PTYs, mouse protocols, paste and selection.
GUI/TUI smoke tests drive real inputs and verify resulting file bytes. A separate
smoke test kills Slate, restarts it and saves recovered unsaved work. Installation
checks run the copied executable and verify the GUI symlink/desktop resources.

SSH and tmux integration fixtures are isolated and mandatory in the Linux CI
job. They can report an explicit skip locally when missing tools or restricted
host facilities prevent execution. The development sandbox blocks SSH privilege
separation and tmux's Unix sockets; their full local workflows were not validated
there. Vim, htop, mouse protocol probes and the actual shell workflows did run.

CI additionally checks the Qt 6.4 public adapter API and runs GUI tests against
Fedora and Debian's compatible Qt/Kirigami packages. Remote CI results must be
checked separately; adding the matrix is not evidence that those remote jobs
have passed. Cargo requirements are compatible ranges, native discovery is a
minimum API requirement, and QML imports are versionless. The dependency-policy
check rejects exact constraints. Lockfile resolutions and locally observed SDK
versions do not impose exact installed Qt/Kirigami runtime versions.
