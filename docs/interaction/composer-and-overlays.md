# Composer And Overlays

## Problem

Search, transcript navigation, approvals, and editing share a terminal keyboard.
Ambiguous input ownership can swallow text or execute a different selection
from the row the user sees.

## Scope And Non-Goals

This specification covers the existing composer, command/model search,
read-only overlays, pickers, and setup editors. Configurable keymaps and a full
editor mode are outside this change.

## Architecture

Terminal events pass through `keymap.rs`, `AppEvent`, and `event_dispatch.rs`.
Presentation state lives in `state/`; renderers consume that state. Search
projection must be shared by row rendering, navigation bounds, and selection.

## Contracts

### INPUT-01: Active Surface Owns The Key

Priority is the top overlay, then a pending interaction's shortcuts with the
scope described below, then ordinary composer and transcript handling. One key
must not both dismiss an overlay and cancel a turn or approve a request.

| Active surface | Keys | Result |
| --- | --- | --- |
| Command palette or model search | Printable characters, including `j` and `k` | Edit the query |
| Command palette or model search | Up/Down | Move the selected result |
| Command palette or model search | Enter | Apply the selected result |
| Command palette or model search | Esc | Close the search surface |
| Non-search list or permission picker | Up/Down or j/k | Move selection; Enter applies |
| Resume picker | Printable characters | Search recent threads; Up/Down selects; Tab cycles sort |
| Help | 1/2/3 | Choose General/Commands/Runtime tab |
| Help Commands tab | Up/Down or j/k; wheel | Move through wrapped command entries |
| Status | 1/2/3 or Left/Right/Tab/BackTab | Change status tab |
| Help, Status, Context | PageUp/PageDown; Home/End | Page through the body; jump to its start/end |
| Help General/Runtime, Status, Context | Up/Down or j/k; wheel | Scroll the body by visual rows |
| Setup text editor | Printable characters, arrows, Home/End, Backspace/Delete | Edit that field; Enter saves, Esc cancels |

The resume picker already has search-specific routing. Searchable surfaces must
not inherit the plain-list j/k shortcuts.

### INPUT-02: Composer Submission And Editing

- Enter submits; Shift+Enter and Ctrl+J insert a newline.
- Ctrl+C closes the top overlay without cancelling underlying work or arming
  quit. Without an overlay, the first press clears an idle composer or requests
  cancellation while running, preserving the running draft. It also shows
  `Press Ctrl-C again to quit` for one second. A second Ctrl+C within that
  window exits, including while cancellation is still draining.
- Ctrl+D participates in the same one-second confirmation only with an empty
  composer and no overlay. Otherwise it deletes forward in an editable field
  or does nothing in a read-only surface; it never inserts a literal `d`.
- Quit confirmation requires the same shortcut twice. Another key, paste,
  mouse button/drag/wheel interaction, or suspension clears the confirmation.
  Passive pointer motion preserves the armed shortcut. Expiry restores
  the ordinary footer without requiring another input event. Reported key
  repeats cannot confirm quit; terminals without repeat metadata remain
  subject to their own key encoding. `/quit` remains an explicit direct exit.
- Unix Ctrl+Z suspends the foreground process group through RUN-07. It never
  edits the active input field; platforms without job control ignore it.
- With no overlay, Esc requests cancellation while running and is otherwise a
  no-op, except for the explicit shell-approval rejection action in RUN-03.
- Up/Down first follow the existing input-history boundary rules, otherwise
  move inside multiline input or scroll when the composer is empty.
- Ctrl+B toggles the sidebar; Alt+T toggles thinking visibility.
- Pasted content uses the paste event path, including large-paste expansion at
  submission; it must not be replayed as individual shortcut key presses.
  Pending paste is applied before interpreting the next pressed/repeated key
  or applying an input action, except palette dismissal described in INPUT-03.
  Immediate submission includes the complete paste, and cursor/history/approval
  routing sees the resulting composer.
  Clearing discards pending bursts, deadlines, placeholder payloads, and the
  current paste-generated notice, while preserving unrelated warnings/status.
  Submission expands and consumes the complete draft through the same cleanup
  boundary, including whitespace-only input; submitted paste notices do not
  linger after their content is sent or discarded.
  Outside the command palette, Esc retains its existing cancellation/no-op
  behavior and preserves the draft. Palette dismissal discards its draft as
  specified in INPUT-03; no paste may appear later in a cleared or submitted
  composer.
- Paste removes escape/control sequences before active-surface routing and
  burst buffering, preserving tabs and normalizing CR/CRLF to one newline.
  Editable overlays receive sanitized text with line breaks converted to
  spaces; read-only ownership remains unchanged. See
  [display text boundary](../features/display-text-boundary.md).
- Unicode bidirectional controls remain in pasted and submitted source, but
  appear as code-point labels such as `⟦U+202E⟧` while editing. Cursor motion and
  deletion treat each label as its original single character, including when a
  label wraps. Joiners, variation selectors, and emoji retain their normal form.
  Credential editors remain masked.

An ordinary composer accepts j/k as text even when empty. Transcript scrolling
uses arrows, PageUp/PageDown, or the mouse. An empty approval composer retains
its explicit navigation shortcuts: arrows select an action, while shell
approvals use PageUp/PageDown and Home/End to inspect the full command and
working directory. A configurable Vim mode is not provided.

Composer rendering, height, cursor placement, scrolling, and Up/Down movement
consume one pure text layout with the actual main-pane width after any sidebar.
The layout cache includes text, width, initial indent, and subsequent indent;
setup editors without indents cannot reuse composer-prefixed rows. Composer
continuations use a two-column indent, including explicit newlines. Tabs retain
the existing four-column expansion. At a soft-wrap boundary before another
character, the cursor belongs to the next displayed row. Moving vertically
chooses the nearest valid character offset on the adjacent displayed row.
The shared grapheme wrapping profile keeps combining sequences and joined emoji
on one row and maps vertical navigation to their character-offset boundaries.
Transcript uses the same range/width primitives with word wrapping; see
[mouse selection](../features/mouse-text-selection.md). This layout guarantee
preserves character-offset storage while horizontal movement and Backspace/Delete
operate on whole graphemes in every shared editable surface. Stale offsets inside
a grapheme snap to its start; insertion and paste snap forward after newly joined
clusters, while deletion snaps back if neighboring clusters join.
Navigation compares untruncated insertion-boundary columns; hardware cursor
clipping must not make the last character indistinguishable from a newline or
end of input. Setup editors retain their single-line clipped rendering and
compute the cursor from that same display text, including masked API keys.
Resize and sidebar toggles recompute the width from the same pane geometry.
Viewport height reservation and palette anchoring measure the bottom pane at
that same main-pane width, not the full terminal width. Each rendered frame
publishes its width for subsequent navigation. Already measured composer rows
are clipped rather than wrapped a second time at degenerate widths.

### INPUT-03: Overlay Lifecycle

- Opening an overlay immediately cancels transcript selection and edge
  autoscroll without copying. Dismissal does not resume the previous drag;
  wheel input belongs to the current overlay or transcript.
- A slash token opens the command palette; adding argument whitespace returns
  to the composer so arguments can be entered explicitly.
- Explicit palette dismissal clears its slash input so it does not reopen
  immediately, using the same complete draft/paste cleanup boundary. Selecting
  a command dismisses the palette before dispatch.
- Palette Esc, Ctrl+C, and direct close preserve their pre-paste dismissal intent:
  discard the pending draft without a preliminary flush that could hide the
  palette. Other keys still route against the complete flushed draft.
- Esc affects the top overlay. Setup cancellation follows the owning setup
  flow; it must not implicitly submit a credential or change permissions.
- Model-search dismissal resets only its query, cursor, and selection. It
  preserves the underlying composer text, cursor, and pending paste content.
- Nested setup/search overlays edit their own field. Closing them restores
  focus to the previous surface without clearing the underlying composer.
- The command palette still clears its slash token on explicit dismissal;
  that token is command input, not an unrelated composer draft.

### INPUT-04: Visible Model Rows Are Selectable Rows

Model search filters available presets by model label or provider label,
ignoring ASCII case. The same ordered result set supplies rendering, arrow-key
bounds, and Enter selection. Enter selects the highlighted model's identity;
an empty result set performs no model selection.

Changing the query resets the selection when necessary. A provider-name match
must not render an empty list while Enter selects an invisible model.

The model query owns its cursor. Left/Right, Home/End, Backspace, and Delete
operate on that query, including insertion before existing Unicode text.
The renderer keeps the query cursor visible when the input is wider than the
available row. Editing the query never edits the underlying composer.

Paste is routed to the active text surface. Single-line search/setup fields
normalize pasted line breaks to spaces and receive the full text directly;
they do not put placeholders into the conversation composer. Read-only
overlays ignore paste. Composer paste retains the existing burst/placeholder
behavior.

### INPUT-05: Model Selection Uses The Existing Setup/Runtime Path

Resolve a selected row using provider family, provider/profile ID, and model
ID. Two endpoints may expose the same model ID; they are distinct choices.
Model search and the existing unified picker share the same action handler:
ready providers request runtime rebuild; providers needing credentials,
configuration, or reasoning options follow the corresponding setup surface.
A local-only configuration change is not a completed runtime switch.
The local Candle choice retains the existing preview-only notice; selection
does not claim that a local backend has finished loading.

The focused fake-port check proves the request or setup transition. Runtime
rebuild and persistence remain covered by their owning maintenance tests; the
fake does not prove that a live provider accepted the new model.

### INPUT-06: Read-Only Overlay Scrolling

Help, Status, and Context keep their tabs and navigation hints outside one
scrollable body. All body content uses the shared styled wrapping and display
sanitization boundary, including long paths, URLs, command descriptions, and
CJK text. Help Runtime presents its sections in reading order rather than
fixed-height columns that can discard content.

The same materialized rows determine rendering and scroll bounds. Offsets use
`usize` and clamp to `content_rows - visible_rows` on both navigation and render;
input refreshes content bounds using the last measured body dimensions. A page
is the visible body height minus one overlapping row, with a minimum of one.
Repeated scrolling at a boundary cannot accumulate debt. Width/height changes
and shrinking runtime content clamp the current numeric row anchor.

Opening an overlay or changing tabs starts at the top. Help Commands keeps its
selected entry visible during entry navigation; page and boundary navigation
keep a visible selection while allowing every wrapped row to be reached.
Overlay navigation never edits the composer or moves the transcript viewport.
Input before the first measured body cannot scroll its rows; command-entry
navigation can still change the selection for the initial frame.

## Validation Matrix

| Contract | Observable check |
| --- | --- |
| INPUT-01 | Dispatch j/k and arrow keys in search; verify the query and rendered results; exercise Help Commands scrolling |
| INPUT-02 | Cursor/history tests plus immediate paste-submit, edit, clear, Esc, and mixed-size paste sequences through production key dispatch; indent cache isolation and rendered vertical movement across sidebar/resize widths |
| Quit shortcuts | Overlay ownership, busy cancellation, same-key confirmation, expiry, input disarming, reported repeats, and footer rendering through production key dispatch |
| Grapheme editing | Shared editor ownership; previous/next whole clusters; Backspace/Delete; stale character offsets; insertion/paste/deletion joining neighboring clusters |
| INPUT-03 | Open and dismiss overlays through key dispatch; verify no runtime cancel command is sent |
| INPUT-04 | Filter by provider; render and select the same model through Enter; verify zero-result behavior |
| INPUT-05 | Select a model and assert rebuild/setup routing; disambiguate endpoint profiles sharing a model ID |
| INPUT-06 | All seven Help/Status/Context bodies at 80x24, 60x20, and 40x12; final-row reachability, CJK/long-line wrapping, entry navigation, immediate scroll clamping, resize/content shrink, and draft/transcript isolation |

## Open Risks

- Resume search retains append/backspace editing; full cursor editing there
  remains a separate follow-up.
- A configurable Vim mode remains outside the current editor contract.
- Large-paste placeholders are not atomic editing elements yet; editing their
  label can prevent expansion on submit. Grapheme-safe editing does not imply
  placeholder-safe editing.
- Snapping a stale explicit cursor offset scans grapheme boundaries up to that
  offset. Repeated reads can be linear in draft length; a shared editor index
  remains separate performance work.

## Source Journals

- [TUI interaction contracts](../journal/2026-09-17-tui-interaction-contracts.md)
- [Input ownership and draft preservation](../journal/2026-09-17-tui-input-ownership.md)
- [Paste input ordering](../journal/2026-10-02-tui-paste-input-order.md)
- [Composer wrap geometry](../journal/2026-10-02-composer-wrap-geometry.md)
- [Display text boundary](../journal/2026-10-03-display-text-boundary.md)
- [Unicode display and editing boundaries](../journal/2026-10-03-unicode-boundaries.md)
- [Interrupt, quit, and Unix job control](../journal/2026-10-03-tui-interrupt-suspend.md)
- [Terminal review follow-up](../journal/2026-10-03-terminal-review-follow-up.md)
- [Bounded read-only overlays](../journal/2026-10-05-overlay-scroll-bounds.md)
