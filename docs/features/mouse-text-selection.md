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

The snapshot retains shared immutable row blocks rather than independently
hashing/stringifying all transcript rows. Area and scroll updates only replace
geometry and shared handles. Unchanged committed blocks retain styled rows,
plain text, and measured widths across frames and appended turns. Width, cwd,
thinking visibility, committed replacement, reset, and restore invalidate the
history layout. The replaceable active block uses complete styled-line equality,
including middle content, styles, and alignment, not an edge-only fingerprint.
An eligible streaming response has its own source-epoch/width/view cache:
stable body blocks are retained, and only the preview/compact summary changes.
Selection consumes the joined history/prefix/response snapshot without copying
its row content. Source replacement and canonical replay cannot retain a stale
response body, including when byte length is unchanged. Selection coordinates
remain numeric; growing a selected row does not automatically extend its endpoint.
Styles do not change copied text. Graphemes wider than the available
transcript row are displayed as a single replacement character rather than
creating invisible selectable content.

Mouse handling uses that latest snapshot to map screen coordinates back to
wrapped transcript rows. The tick loop drives edge autoscroll while dragging.

Transcript scroll state explicitly distinguishes following the tail from an
absolute top visual-row anchor. Rendering publishes the current wrapped row
count and transcript dimensions to the numeric state model; scroll input
refreshes those bounds before applying its delta. State modules do not build
styled lines or terminal layout objects. A manual anchor remains fixed when
visual rows are appended, while tail-following uses the newly measured end.
Layout changes clamp an anchor without implicitly enabling tail-following.
Reaching the bottom through manual scrolling restores tail-following.

Offsets, viewport slices, and selection row indices use `usize`. Only local
terminal coordinates use `u16`; a long transcript must not be passed through
`Paragraph::scroll`. The existing one-row breathing room at the tail remains.
Reset and thread restoration explicitly return to tail-following. Scroll input
before the first measured transcript frame is ignored.

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
- Keyboard, wheel, and drag autoscroll clamp every delta to the current visual
  rows. Overscrolling cannot accumulate invisible scroll debt.
- An up-scrolled view stays on its top visual row during append-only streaming;
  width changes retain that numeric anchor subject to the new bounds, not a
  semantic text-location anchor across reflow or content replacement.
- Rendering, highlight, and copy remain reachable beyond 65,535 visual rows.
- Clipboard failures must not terminate the TUI; they surface as notices.

## Validation Matrix

| Behavior | Validation |
| --- | --- |
| Wrapped text range extraction | Production viewport buffers and `TranscriptSelection` across narrow/wide widths, prose, CJK, emoji, combining marks, tabs, and URLs |
| Exact rows and partial scrolling | Counted rows equal materialized/rendered rows; tail and partial-window buffer assertions |
| Autoscroll selection extension | Unit tests for non-zero scroll offset |
| Scroll bounds and tail-following | Pure state tests for extreme deltas, empty/short content, layout changes, appends, and return to tail |
| Stable streaming anchor | Production key dispatch followed by appended stream deltas; visible buffer rows remain unchanged |
| Long transcript reachability | Production renderer with 70,000 rows; tail buffer, highlight, and copied text agree |
| Shared snapshot reuse | Work counts for unchanged frames, scroll, copy, and streamed tails; retained allocations on committed append |
| Active stream snapshot | Retained stable body allocations; current preview copy after drag extension; full/compact, suppression, thinking, and finalization transitions |
| Snapshot refresh | Same-sized middle replacement and full styled-tail invalidation; width/cwd/visibility/reset/restore guards |
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
- [2026-10-02-transcript-scroll-anchors](../journal/2026-10-02-transcript-scroll-anchors.md)
- [2026-10-03-transcript-row-reuse](../journal/2026-10-03-transcript-row-reuse.md)
- [2026-10-03-active-stream-rows](../journal/2026-10-03-active-stream-rows.md)
