# Daily editing milestone

Both frontends use the same commands, preferences, documents, views, layouts,
search rules and terminal resources. GUI controls and TUI prompts expose that
shared behavior. The native adapter remains a presenter; Rust owns editing,
recovery and process control.

## Updates and frontend boundary

Rust remains authoritative for documents, history, view state and services.
`session.rs` owns startup composition, `service_events.rs` applies worker replies,
and `SaveWorkflow` and `RecoveryState` group their lifecycle state outside the
main `App` field list. `recovery_io.rs` owns the checkpoint storage format. The
Qt Quick frontend owns graphical controls, text shaping and pixel hit testing;
Ratatui owns terminal presentation. Neither frontend has a second mutable editor
buffer or independent undo stack.

Worker replies and PTY output notify a coalesced event signal. On Unix a
nonblocking socket pair wakes Qt through QSocketNotifier; a condition variable
wakes the TUI alongside input from its reader thread. Workers never mutate Qt or
App state. Qt coalesces output bursts over 8 ms. A separate one-second maintenance
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
uses only its destination, whose source is already removed. Bulk actions execute
at the resolved repository root with a pathspec scoped to the workspace subtree.
Commits include all staged repository changes. Section staging collects only
that group's paths into a single job. Unstaging an unborn index removes cached entries
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

Editor and terminal content use independent shared-core palettes. Dark defaults
are #1b1e26 for editors and #14171c for terminals; native GUI chrome follows Qt.
`editor_theme` chooses auto/dark/light, and `terminal_theme` additionally accepts
editor to inherit its palette. Automatic palettes retain Qt's latest view/text/
selection colors. The editor's background luminance chooses the syntax palette.
Terminal ANSI and selection colors come from its own palette; explicit RGB
colors supplied by applications pass through unchanged. The minimap darkens its
editor background. Palette changes invalidate the appropriate render cache.
GUI tabs fit their header; when space is limited, the active tab and a menu of
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
300 bytes of text payload, plus entry metadata. Documents use persistent ropey
ropes, so background snapshots share their storage and edits copy affected chunks.

File open/save jobs have their own queue, independent of Git and highlighting.
Saving captures the buffer and original disk baseline. Completion updates the
save baseline/path without replacing the current buffer; edits made after the
request stay dirty. Concurrent saves of one document and closure during a save
are rejected. Duplicate opens reuse a document without losing unsaved changes.
Saves preserve permissions and symlink targets, recheck external changes and use
atomic replacement/no-clobber persistence. Files and containing directories are
synced on Unix. These checks detect ordinary external edits; the filesystem does
not provide a universal compare-and-swap against every concurrent external writer.

Buffers always hold `\n` lines. `text_format` decodes UTF-8 (BOM), UTF-16 (BOM)
and legacy encodings through encoding_rs, records the dominant line ending and
re-encodes on save; documents keep a SHA-256 baseline of the bytes last read or
written, so conflict checks work for any encoding or mixed line endings.
`fsio::write_file` chooses atomic replacement when the replacement can carry the
original owner, group and extended attributes, and writes in place for hard
links, unwritable directories, or when ownership cannot be reproduced. Read-only
and permission errors are typed so frontends can ask for confirmation or a
the validated write helper through sudo (terminal, with the terminal handed over)
or pkexec (GUI, on an independent worker).

## Workspace recovery

Each canonical root owns a checkpoint and advisory lock. One instance writes
recovery for that root. JSON contains the text and save baselines of unsaved
documents (clean documents are path references), independent views,
layout/tab/focus state, browser directory and terminal working directories.
Single-file sessions keep no checkpoint unless `file_recovery` is enabled; then
the checkpoint is keyed by the file. A fingerprint of documents' generations,
views and layout skips writes when nothing recorded has changed.
Small buffers stay inline; larger buffers and their saved text are streamed into
private, content-addressed files under `buffers/`, each bounded by the 1 GiB
editor limit. SHA-256 and byte counts are checked before restoring each rope.
The 128 MiB JSON cap applies to metadata, not the aggregate buffer contents.
External references use schema 2; schema 1 inline checkpoints remain supported.
Payloads and their directory are synced before the metadata is atomically
replaced. Failed writes retain the previous checkpoint. Corrupt/missing payloads
are excluded during salvage while intact documents retain their text and views;
the original metadata is kept beside the salvaged session. Payload cleanup runs
only after commit and preserves references from retained previous/damaged JSON
checkpoints; unreadable retained metadata disables cleanup.
`RecoveryState` tracks submitted and persisted fingerprints separately, with
at most one checkpoint in flight. A typed completion acknowledges the write;
failures retain a visible warning and dirty state for retry even without another
edit. The warning is combined with ordinary status messages in both frontends
until persistence succeeds. Edits made while writing trigger another checkpoint.
The maintenance wake queues changed state at most once a second; shutdown drains replies while submitting and waiting for its I/O barrier,
then flushes pending I/O
and the final checkpoint before releasing the lock. Recovery does not save user
files or replay shell commands.

Restore validates schema, size, layout, resource references, ID bounds and UTF-8
cursor offsets before replacing live state. Clean buffers follow current disk
content; dirty buffers keep their original baseline so saving still detects a
conflict. A clean file deleted meanwhile reopens as a new, empty file. Discard-and-quit clamps views to the
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
checks run the copied executables, verify that `slate` does not link Qt and that
`slate-gui` and the desktop resources are installed. `tests/tui_nano.rs` drives
the real terminal executable in a PTY through a VT emulator for nano workflows.

SSH and tmux integration fixtures are isolated and mandatory in the Linux CI
job. They can report an explicit skip locally when missing tools or restricted
host facilities prevent execution. The development sandbox blocks SSH privilege
separation and tmux's Unix sockets; their full local workflows were not validated
there. Vim, htop, mouse protocol probes and the actual shell workflows did run.

CI additionally checks the Qt 6.4 public adapter API and runs GUI tests against
Fedora and Debian's compatible Qt packages, without KDE frameworks. Remote CI results must be
checked separately; adding the matrix is not evidence that those remote jobs
have passed. Cargo requirements are compatible ranges, native discovery is a
minimum API requirement, and QML imports are versionless. The dependency-policy
check rejects exact constraints. Lockfile resolutions and locally observed SDK
versions do not impose exact installed Qt runtime versions.

## Desktop editor (GUI)

The GUI depends only on Qt Quick Controls. A `Theme` singleton maps the
platform `SystemPalette` to the colours the QML uses, and an `image://icon/`
provider renders freedesktop theme icons tinted like text; when the platform
names no icon theme (offscreen, other desktops) an installed one is chosen.
Menus read the shared action catalog when they open and the Git pane only when
repository state or pane kinds change, so typing never queries the catalog; the
desktop smoke test asserts this.

Editor rows are cached `QTextLayout`s painted opaquely. Wheel and touchpad input
accumulate pixel deltas: whole rows scroll the core view, the remainder offsets
the painted rows, and the core renders one extra row so the offset never shows
a gap. Horizontal scrolling uses the core's `left` column, which the cursor
only reclaims after it moves. The minimap paints a sampled outline (indent and
length per line) that the core caches per document generation; the horizontal
scroll range covers every line.

The external-change watcher runs on the IO worker once a second: it compares
each file's size, modification time and inode with the document's stamp and
rereads only changed files, comparing their hash with the save baseline, so
`touch` is not a change. Clean documents reload; modified ones queue a
`file-changed` prompt that either reloads or adopts the new baseline.

`slate-gui FILE` hands files to a running window over a per-user Unix socket in
`XDG_RUNTIME_DIR`; the running window opens them (at their `+LINE`) and raises
itself. A signal thread turns SIGTERM/SIGHUP/SIGINT into an orderly Qt quit, so
recovery state is flushed and the socket removed. Other platforms wake the Qt
loop through a callback instead of the Unix event descriptor, and CMake reports
the Qt libraries for Cargo to link.

## Text model and highlighting

Documents are `ropey` ropes; searching walks the rope directly
(`regex-cursor`), and line, column and UTF-16 conversions come from the rope's
indexes, so large files and long lines stay cheap. Each edit appends to two
bounded feeds. One records line-level changes, which the highlighter uses. The
other records LSP `TextChange`s in the coordinates the server expects. An
overflowing feed turns into "everything changed".

The highlight worker keeps syntect parse states every 32 lines per document.
A request names the visible rows and the first edited line; the worker resumes
from the nearest valid checkpoint and drops work superseded by a newer
request. The core shifts its per-line span cache with each edit; stale colours
stay visible until fresh ones arrive.

## IDE services

`ide::Event` is the bus between the core and its services. Language servers,
debug adapters and tasks are child processes; reader threads parse their
output and post typed events on the reply channel, and the main loop applies
them. Language servers and debug adapters share `rpc.rs`, which handles
Content-Length framing with separate reader and writer threads.

The LSP client starts servers per (language, project root) and opens
documents when they first appear. It syncs incrementally when the server
supports it and falls back to full text when the feed overflowed or the
document was replaced (reload, recovery). Diagnostics are kept per file. They
move with local edits until the server publishes again, and task problem
matchers add their own (`source = "task"`). Completion results are filtered
locally with `nucleo-matcher` while typing; incomplete lists ask again.
WorkspaceEdits change open documents through `Document::replace_many`, which
groups its revisions so one undo reverts the whole edit. Closed files open as
unsaved background documents, so changes can be reviewed, undone and saved.

The debugger client speaks DAP to `gdb -i dap` by default. It sends
breakpoints and `configurationDone` after `initialized`. On `stopped` it
requests the stack, scopes and variables and moves the editor to the paused
line; expressions are evaluated in the `watch` context, because GDB's `repl`
context runs commands.

Workspace trust gates every service that runs code from the folder: language
servers, formatters, tasks, debugging, project tools and Git. Git is gated as a
whole, not just staging: `git status` alone can run a repository's clean
filters and fsmonitor hook, so no Git process starts until the repository's
folder is trusted (finding the repository walks up to `.git` without running
Git). Trust is decided for the folder a service would run in. The most
specific `trusted-folders` entry wins, so a `!` entry restricts a folder inside
a trusted one; `App::trusted_path` caches the decision until trust changes. A
language server for a file outside the trusted workspace, a formatter, or Git
for a repository that contains the workspace each ask about their own folder
and retry the action once trusted. Git status and bulk staging are scoped to
the workspace's subtree of a parent repository.

Child processes that Slate waits for (Git, formatters, tools) run in their own
process group with a deadline (`process::run`); output is limited to 16 MiB per stream, and the deadline includes input and
output pipes inherited by descendants. On timeout the whole group is
killed, so a hung `git` or a formatter's grandchildren do not linger. Tasks,
language servers and debug adapters are stopped the same way when they end or
Slate quits.

Edits computed elsewhere (formatters, rename, code actions, tools) are tagged
with the document's content version when requested and refused if the document
changed meanwhile. Workspace edits validate the whole plan, and refuse files
outside the workspace, before changing anything; closed files open as unsaved
background tabs so the change can be reviewed and undone. Edits applied to a
document rebase every view's carets, snippet fields, folds, breakpoints and
navigation history in one pass (`App::replace_in_document`).

Configuration files are read through `config::read`, which distinguishes a
missing file from one that does not parse; parse errors reach the status bar,
and files that failed to parse (`settings.toml`, `layouts.toml`) are never
overwritten. Small private files are written atomically with
`fsio::write_private` (temporary file, fsync, rename, directory fsync).

## Layout integrity

Closing a pane moves its documents and terminals into the remaining layout.
Applying a preset or a saved layout rebinds the running shells and open
editors to the new slots, and anything left over is placed in it. No document
or shell is ever hidden without a tab. The saved-layout limits (32 tabs per
pane, depth 12) hold while editing: full panes spill into others or a new
split. A checkpoint that fails validation is salvaged (every buffer that still
parses, views clamped, a plain layout if needed), and the damaged file is kept
beside it.

## Terminals and Git workers

vt100 callbacks answer cursor and device queries in order, take window titles
and OSC 52 copies, and follow OSC 7 directories. Terminal input is written by its own thread through `InputQueue`. Byte
reservations include the current write until it completes; a 4 MiB budget and
1024-message channel reject overload without blocking. Bracketed paste is one
message, so rejection cannot leave the child in paste mode. Write failures and
refused protocol replies are surfaced through the main loop. Closing a session
closes its input queue before stopping the child, and queued reservations are
released as messages are dropped. Exited
shells are reaped and their tabs closed.

Git runs on two workers: reads (status, diff) and writes (stage, commit). The
app keeps at most one status read in flight and coalesces requests. Every git
process has a timeout and its own process group. Paths keep their raw bytes,
so files with non-UTF-8 names can be staged.

## Save operations and reload generations

A save retains an immutable document snapshot. Preparing it resolves the actual
Save As path and captures the expected destination baseline before any write.
Permission/read-only failures return this prepared snapshot; confirmations are
queued by document and retries preserve the path, contents and baseline. They
never take their target from the currently focused tab. New edits stay dirty
when an earlier snapshot completes. Save All retains a sequential document queue
through untitled-path dialogs and permissions. Cancelling or failing a save stops
that workflow. Named documents use their own format-on-save settings in Save All.
Saving a modified tab closes it only after a successful write and
only when no newer edits remain. Native file dialogs explicitly suspend and
complete their core prompt, binding Save As to its original document.

Reload requests carry the requested content version. Completion cannot replace a
document edited in the meantime. Document reads reject special files and enforce
the byte limit while reading, rather than relying only on a metadata size check.
The opened file descriptor is checked, and Unix opens use nonblocking mode so a
path replaced with a FIFO cannot stall the worker. Save As applies the same
regular-file and size checks before reading an overwrite baseline.

## Service budgets and large workloads

Service requests and replies use bounded channels; request submission reports
overload rather than waiting on a worker. Recovery barriers drain replies while
waiting and have a deadline. A frontend processes at most 128 replies
or four milliseconds of work per pass and schedules another wake for remaining
work. RPC output is bounded by both message count and bytes; overload disconnects
a stuck child without blocking the input thread. LSP and DAP requests expire after
30 seconds; LSP cancellation is sent and initialization failures stop the server.
File operations and privileged authentication do not share a blocking execution
path. Large fuzzy pickers use a latest-query worker with cancellation. Workspace
search queues only its newest request and materializes shared rope snapshots on
a worker. Wrapped rows are cached by document generation, width and tab width,
with limits on retained entries and rows. Syntax parsing skips individual lines
larger than 64 KiB, preserving plain text editing for generated/minified files.
Large-document minimaps and horizontal width measurements run on a coalescing
worker and retain the previous outline until the current one arrives. Language
detection reads only a bounded first-line hint. LSP synchronization is limited
to 8 MiB per document; larger documents remain editable without a server. RPC
messages and queued output are capped at 16 MiB, including pre-initialization
queues, with at most 1024 outstanding LSP requests.

## Profiles, explorer and accessibility

Profiles live at `$XDG_CONFIG_HOME/slate/profiles/NAME.toml`; each contains base
preferences, layout shape and editor-only mode. Built-ins are default, minimal and
workspace. The selected profile is recorded in settings.toml. Overrides resolve
as profile, global editor, workspace editor, global language, workspace language.
Global overrides live in editor-overrides.toml and workspace overrides in
`.slate/settings.toml`. Only typed editor preferences are accepted in overrides;
these files cannot add commands, executables, terminal clipboard permissions or
key bindings. GUI settings show effective values and their override sources.
Rendering resolves document-specific options, including when split panes show
different languages.

Both frontends use the same asynchronous create, rename and Trash operations.
The GUI flattens only expanded directories into an indented, scrollable model;
the TUI keeps directory navigation with Enter/Right and Left. Both file panes
stay within the workspace root: parent navigation stops there, directory symlinks
cannot browse outside it, and recovery resets an out-of-workspace browser location
to the root. Explicit file opening remains available for files elsewhere. Rename
refuses to replace an existing destination (atomically on Linux) and updates open documents.
Trash uses the desktop's gio implementation and never falls back to permanent
delete. Modified documents must be saved or closed first. Directory/layout GUI
windows can receive forwarded files even though those launch options themselves
still create a separate window.

Assistive technology gets document-wide UTF-16 counts and ranges backed by rope
indices, separately from the small IME context. Accessible cursor/selection and
scroll operations use core commands. Character geometry and pointer offsets use
the same visible rows and text layout as painting. AT-SPI tests use a private
D-Bus session on a real display backend; CI also requires KDE style, X11/Wayland
and native-dialog coverage, plus Orca discovery through the private accessibility
bus. Screen-reader speech output still requires listening
with an actual audio setup and is not asserted by automated tests.
