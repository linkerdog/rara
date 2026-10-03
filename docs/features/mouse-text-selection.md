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

Transcript rendering first materializes styled visual rows through the shared
text layout boundary. Word wrapping is the transcript profile; grapheme wrapping
with explicit indents is the composer profile. Both profiles measure display
columns, keep grapheme clusters indivisible, and expand tabs to four spaces.
Row counting, viewport slicing, and selection consume those materialized rows;
the renderer must not wrap them again. Soft-wrap word separators are omitted
from display and copied text, while explicit blank lines are retained.

Snapshot rebuilding is guarded by the viewport area, scroll offset, and complete
visual-row content. Unchanged frames reuse the previous screen-area-to-text
mapping. Styles do not change copied text. Graphemes wider than the available
transcript row are displayed as a single replacement character rather than
creating invisible selectable content.

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
- Selection endpoints snap to whole graphemes, including combining sequences
  and joined emoji; highlight and copy must cover the same terminal cells.
- Transcript row counts include exactly the rows that rendering can display,
  including wrapped prose, long tokens, URLs, and explicit empty rows.
- Edge autoscroll only starts once the cursor leaves the transcript viewport and
  uses the same transcript scroll direction as wheel and keyboard scrolling.
- Clipboard failures must not terminate the TUI; they surface as notices.

## Validation Matrix

| Behavior | Validation |
| --- | --- |
| Wrapped text range extraction | Production viewport buffers and `TranscriptSelection` across narrow/wide widths, prose, CJK, emoji, combining marks, tabs, and URLs |
| Exact rows and partial scrolling | Counted rows equal materialized/rendered rows; tail and partial-window buffer assertions |
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

- Emoji display width depends on terminal policy; the application uses the
  pinned `unicode-width` policy consistently across layout and selection.
- The transcript snapshot is frame-based. If a mouse event arrives before the
  first transcript frame, selection start is ignored.

## Source Journals

- [2026-05-13-transcript-copy-selection](../journal/2026-05-13-transcript-copy-selection.md)
- [2026-10-02-shared-transcript-wrapping](../journal/2026-10-02-shared-transcript-wrapping.md)
