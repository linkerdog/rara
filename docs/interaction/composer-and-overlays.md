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
| General help | 1/2/3 | Choose General/Commands/Runtime tab |
| Help Commands tab | Up/Down | Move through command entries |
| Status | 1/2/3 or Left/Right/Tab/BackTab | Change status tab |
| Context | Up/Down or j/k; PageUp/PageDown | Scroll context |
| Setup text editor | Printable characters, arrows, Home/End, Backspace/Delete | Edit that field; Enter saves, Esc cancels |

The resume picker already has search-specific routing. Searchable surfaces must
not inherit the plain-list j/k shortcuts.

### INPUT-02: Composer Submission And Editing

- Enter submits; Shift+Enter and Ctrl+J insert a newline.
- Ctrl+C clears an idle composer, or requests cancellation while running.
- With no overlay, Esc requests cancellation while running and is otherwise a
  no-op, except for the explicit shell-approval rejection action in RUN-03.
- Up/Down first follow the existing input-history boundary rules, otherwise
  move inside multiline input or scroll when the composer is empty.
- Ctrl+B toggles the sidebar; Alt+T toggles thinking visibility.
- Pasted content uses the paste event path, including large-paste expansion at
  submission; it must not be replayed as individual shortcut key presses.

An ordinary composer accepts j/k as text even when empty. Transcript scrolling
uses arrows, PageUp/PageDown, or the mouse. An empty approval composer retains
its explicit navigation shortcuts. A configurable Vim mode is not provided.

### INPUT-03: Overlay Lifecycle

- A slash token opens the command palette; adding argument whitespace returns
  to the composer so arguments can be entered explicitly.
- Explicit palette dismissal clears its slash input so it does not reopen
  immediately. Selecting a command dismisses the palette before dispatch.
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

## Validation Matrix

| Contract | Observable check |
| --- | --- |
| INPUT-01 | Dispatch j/k and arrow keys in search; verify the query and rendered results; exercise Help Commands scrolling |
| INPUT-02 | Existing cursor, history, newline, paste, and busy-input tests |
| INPUT-03 | Open and dismiss overlays through key dispatch; verify no runtime cancel command is sent |
| INPUT-04 | Filter by provider; render and select the same model through Enter; verify zero-result behavior |
| INPUT-05 | Select a model and assert rebuild/setup routing; disambiguate endpoint profiles sharing a model ID |

## Open Risks

- Help General and Runtime do not yet support scrolling. Narrow/short terminal
  acceptance needs clipping tests and an explicit scrolling design.
- Resume search retains append/backspace editing; full cursor editing there
  remains a separate follow-up.
- Grapheme-cluster editing and a configurable Vim mode are outside the current
  character-offset editor contract.

## Source Journals

- [TUI interaction contracts](../journal/2026-09-17-tui-interaction-contracts.md)
- [Input ownership and draft preservation](../journal/2026-09-17-tui-input-ownership.md)
