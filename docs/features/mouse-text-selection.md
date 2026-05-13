# Mouse Text Selection

## Problem

RARA enables terminal mouse capture so the TUI can receive wheel events and keep
transcript scrolling inside the application. That disables terminal-native
drag-to-select behavior for the visible transcript.

Codex avoids this by keeping transcript history in terminal scrollback and not
capturing mouse events by default. RARA intentionally keeps the application-owned
transcript viewport for now, so copy-friendly selection must be implemented at
the TUI layer.

## Scope

The canonical behavior is application-owned selection for the visible transcript
area only:

- left-button drag starts and updates a transcript selection;
- selected cells are highlighted by RARA while dragging;
- releasing the left button copies the selected plain text;
- dragging outside the top or bottom edge autoscrolls the transcript and
  extends the selection;
- normal wheel scrolling remains available outside active drag selection.

## Non-Goals

- Selecting text in the composer.
- Selecting text in overlays such as `/help`, `/status`, pickers, or context
  inspection.
- Selecting text from the wide-screen sidebar.
- Selecting off-screen terminal scrollback rows outside the transcript model.
- Preserving rich styles in the copied text.

## Architecture

RARA keeps `EnableMouseCapture` enabled and routes left-button mouse events into
`TranscriptSelection`.

The render path owns the authoritative visible transcript snapshot. Each frame:

1. builds the transcript viewport;
2. computes the visible wrapped rows for the current scroll offset;
3. stores the screen-area-to-text mapping in `TuiApp.transcript_selection`;
4. renders the transcript;
5. applies selection highlight over the rendered buffer.

Snapshot rebuilding is guarded by a lightweight key derived from the viewport
area, scroll offset, transcript size, and transcript edge content. Unchanged
frames reuse the previous screen-area-to-text mapping instead of reallocating
wrapped rows.

Mouse handling uses that latest snapshot to map screen coordinates back to
wrapped transcript rows. The tick loop drives edge autoscroll while dragging.

Clipboard output first emits OSC 52 so SSH sessions can copy to the local
terminal clipboard when the terminal permits it. Platform clipboard commands are
best-effort fallbacks for local sessions.

## Contracts

- Selection only starts when there is no active overlay and the mouse down event
  lands inside the transcript snapshot.
- Dragging outside the transcript area clamps to the nearest visible transcript
  row.
- A zero-width selection does not copy anything.
- Copied text is plain text reconstructed from visible wrapped transcript rows.
- Edge autoscroll only starts once the cursor leaves the transcript viewport and
  uses the same transcript scroll direction as wheel and keyboard scrolling.
- Clipboard failures must not terminate the TUI; they surface as notices.

## Validation Matrix

| Behavior | Validation |
| --- | --- |
| Wrapped text range extraction | Unit tests for `TranscriptSelection` |
| Autoscroll selection extension | Unit tests for non-zero scroll offset |
| Mouse event routing | Existing TUI event tests plus focused selection events |
| Clipboard fallback safety | Manual SSH/local verification |
| Render highlight | Manual TUI verification; future snapshot if styling changes |

## Operational Notes

OSC 52 depends on terminal policy. Some terminals disable remote clipboard
writes by default, and tmux/screen may require passthrough support. RARA still
attempts native clipboard fallback, but over SSH that fallback writes the remote
machine clipboard rather than the user's local desktop clipboard.

## Open Risks

- The first implementation reconstructs copied text from wrapped visual rows,
  so extremely wide Unicode grapheme clusters may not match terminal emulator
  selection exactly.
- The transcript snapshot is frame-based. If a mouse event arrives before the
  first transcript frame, selection start is ignored.

## Source Journals

- [2026-05-13-transcript-copy-selection](../journal/2026-05-13-transcript-copy-selection.md)
