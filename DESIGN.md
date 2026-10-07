# Daily editing milestone

Both frontends use the same commands, preferences, documents, views, layouts,
search rules and terminal resources. GUI controls and TUI prompts expose that
shared behavior. The native adapter remains a presenter; Rust owns editing,
recovery and process control.

## Updates and frontend boundary

Rust remains authoritative for documents, history, view state and services. The
Kirigami frontend owns graphical controls, text shaping and pixel hit testing;
Ratatui owns terminal presentation. Neither frontend has a second mutable editor
buffer or independent undo stack.

Worker replies and PTY output notify a coalesced event signal. On Unix a
nonblocking socket pair wakes Qt through QSocketNotifier; a condition variable
wakes the TUI alongside input from its reader thread. Workers never mutate Qt or
App state. Qt coalesces output bursts over 16 ms. A separate one-second maintenance
wake services checkpoints and provides the non-Unix fallback. Rendering is not
responsible for keeping background work alive.

File browsing, Git, document I/O/checkpoints and syntax parsing have separate
workers. A slow Git command or hook cannot stall browsing, opening or editing.

The GUI adapter checks the application revision and viewport before constructing
presentation data. Unchanged requests return only an unchanged marker. Changed
requests carry pane metadata and only changed screen/file/Git components; omitted
components retain their previous frontend state. Screens are cached by document,
view, syntax, theme, viewport or terminal revision. Cursor/focus changes do not
invalidate unrelated screens. Revision sampling happens before constructing a
frame, so output arriving during construction cannot be consumed without a later
update. Qt list models notify QML about list changes, and CellViews repaint from
their own pane changes rather than a global repaint signal.

The TUI rebuilds a snapshot/draws only after input, viewport changes or background
notifications. An idle maintenance wake does not redraw. Qt palette changes send
theme commands, rather than querying and serializing the palette every frame.
Passive terminal hover is ignored unless the terminal application requests
all-motion reporting, so repeated compositor hover events do not invalidate state.

## Commands and Git

One Rust action catalog supplies identifiers, readable names, descriptions,
argument hints, availability reasons and configured shortcuts to both palettes.
GUI and TUI palette searches invoke the same actions. Existing textual commands
remain available with a leading colon. Open, Save As, commit and layout actions
use shared argument prompts; GUI settings offers direct persistent controls.

Git status is parsed using NUL-delimited records, retaining rename source paths.
Index and worktree changes are separate selectable entries, with groups for
staged, unstaged, untracked and conflicts. Staging a working rename and
unstaging an index rename include both names; restaging edits to an index rename
uses only its destination, whose source is already removed. Bulk actions operate at the resolved repository root, including
when the workspace opens a subdirectory. Section staging collects only that
group's paths into a single job. Unstaging an unborn index removes cached entries
recursively, preserving all working files. Operations remain asynchronous and
expose busy/error state. The GUI keeps the commit draft above pane delegates,
which can be recreated by compact layouts; completion clears it only on success. Diff
comparisons use --cached for staged changes, plain diff for unstaged changes,
and --no-index against an empty file for untracked previews. Initial commits do
not depend on HEAD existing. Previews are named read-only documents with bounded
output; they cannot acquire edit history, dirty state or overwrite a source file.

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

An incrementally maintained byte-offset line index avoids scanning the document
prefix for viewport and vertical-position queries. Dirty comparisons are cached
per generation and literal-search regexes are cached per query. Contiguous typing
within 750 ms groups into one undo operation; navigation, selection, paste and
save requests break that group. The public document text accessor is read-only,
so mutations pass through the history/index maintenance code.

The GUI uses cached QTextLayout rows for shaping, styled spans and pixel-to-text
hit testing. Input-method preedit stays local; committed replacements use UTF-16
coordinates translated by the shared core. Surrounding text, cursor and selection
context are exposed to Qt input methods. The terminal retains its fixed-cell
renderer. This provides basic accessible names/roles; complete platform screen
reader integration is not claimed by the input-method tests.

The GUI editor's base/text/selection colors follow Qt's desktop palette by
default. Syntax colors choose a light or dark palette from its luminance.
The latest desktop palette is retained when settings reload or change, including
when switching back from an explicit light/dark theme to automatic colors.
Terminal ANSI colors remain consistent with the emulator. GUI tabs fit their header; when space is limited, the active tab and a menu of
all tabs replace the full tab row. The tab model changes only when titles or
active tabs change, so editing does not recreate controls. Git views receive
repository/selection metadata without editor or PTY screen payloads. File/Git
lists reserve separate scrollbar
gutters. Buttons own their measured background and caption to avoid native style
insets clipping controls or painting captions twice. Git controls wrap at narrow
widths, and a compact commit dialog preserves list space in short panes. Toolbar
actions collapse into the menu. Focus borders, active tab
weight, cursor location and context-sensitive shortcut hints make state explicit.
The TUI presents equivalent prompts with keyboard controls and the same shared
rendered cells. Settings can force light/dark colors or remap scoped shortcuts.

## Startup presentation

The core chooses editor-only or workspace presentation from persistent file and
directory defaults, with an optional CLI override. The selection precedes
recovery and stays authoritative after recovery: requested files do not inherit
an old session's tool focus. Editor-only presentation projects one editor pane
from the complete split tree; it does not replace that tree or discard views.
The shared toggle action therefore preserves tabs, geometry, cursors, history
and running terminals. Splitting or explicitly opening a tool expands the
workspace. Pane cycling cannot focus hidden tools while editor-only is active.

Terminal resource IDs and remembered working directories can exist without a
process. Editor-only startup and recovery defer shell creation until expansion.
Checkpoints include both live and deferred terminal resources, preserving valid
layout references. Recovery in workspace mode prepares shells before replacing
the current model, so a spawn failure leaves the current workspace usable.

GUI command inputs resolve to shared Rust commands before dispatch, including
configured key bindings and raw palette input. Menus and Git controls obtain
availability and effective shortcuts from the same scoped command catalog;
queries for another pane or Git row never change application focus. Qt caches
catalogs by state revision and keeps row catalogs until Git state or configured
global bindings change. Catalogs remain outside editor frame payloads.

The GUI adapter requests quit/discard confirmation before executing destructive
commands, regardless of input route. Only dialog acceptance supplies a confirmed
request. Global Qt shortcuts are projected from Rust preferences and remain
available in tool input fields, while modal dialogs retain their own input.
File pickers, settings and dirty-tab confirmations continue to follow shared
backend prompts; ordinary terminal keys and text-field editing retain their
normal behavior.

GUI startup controls and the TUI settings list use the same preferences and
configuration commands. Settings changes affect subsequent launches; presentation
toggles never write startup preferences. Configured key maps from older releases
receive the new default bindings without replacing explicit assignments.
Configuration paths and state paths share XDG resolution, rejecting relative
base paths and treating empty values as unset.

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
The maintenance wake queues changed state at most once a second; shutdown flushes pending I/O
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
