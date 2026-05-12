# Transcript Copy Selection

## What Changed

RARA now treats mouse drag selection as an application-owned transcript feature
instead of trying to temporarily restore terminal-native selection.

The implementation is intentionally scoped to the visible transcript area:

- left-button drag starts and updates selection;
- selected transcript cells are highlighted in the rendered buffer;
- left-button release copies selected plain text;
- dragging outside the transcript top or bottom edge autoscrolls and extends
  the selection;
- overlays, composer, and sidebar are out of scope.

## Why

RARA captures mouse events to support transcript wheel scrolling. That prevents
the terminal from performing native drag-to-select. Codex avoids the conflict by
using terminal scrollback for transcript history, while opencode-style TUIs keep
mouse capture and simulate selection inside the app.

The selected direction keeps RARA's existing transcript viewport and adds a
copy-friendly path without giving up mouse scrolling.

## Trade-Offs

The copied text is reconstructed from RARA's rendered transcript model rather
than from the terminal emulator. This is more predictable for application-owned
content but does not cover terminal scrollback or UI surfaces outside the
transcript.

Clipboard writes first use OSC 52 for SSH compatibility, then best-effort local
platform commands. Remote clipboard behavior still depends on terminal and
multiplexer policy.

Snapshot rebuilds are keyed by viewport and transcript metadata so stable frames
reuse the previous mapping. This keeps the selection path out of the hot render
loop unless the transcript, size, or scroll position changes.

## Remaining Work

- Add an opt-out config if users prefer no copy-on-select behavior.
- Consider extending selection tests for wide Unicode grapheme behavior.
- Manually verify OSC 52 behavior under tmux, GNU Screen, and common terminals.
