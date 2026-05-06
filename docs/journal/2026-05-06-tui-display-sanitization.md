# 2026-05-06 · TUI display sanitization boundary

## Context

Agent output could visibly corrupt the terminal transcript when model text or
tool summaries contained terminal control characters. The user-visible symptom
was stale characters and misaligned words in `Agent`, `Running`, and approval
sections after mixed progress output and shell approval completion.

The root issue was not one specific card. Multiple render paths could receive
raw text:

- streaming agent deltas;
- committed agent messages;
- live progress events such as Thinking, Exploring, Planning, and Running;
- terminal event output previews;
- inline history insertion.

If a raw carriage return, ANSI/OSC escape sequence, backspace, bell, or other
control character reached terminal printing, it could move the cursor or rewrite
cells outside Ratatui's normal buffer diff.

## Decision

Add one TUI display-sanitization boundary and route display text through it
before rendering. The sanitizer preserves visible text and line boundaries, but
removes terminal side effects.

The code-level rule is:

- raw transcript or tool payloads may remain available for persistence or full
  inspection;
- anything entering markdown display collectors, committed message rendering,
  terminal-event display previews, or inline terminal `Print` must use
  `tui::display_sanitize`;
- inline history insertion sanitizes the full `Line` before width/row
  calculation and printing, so wrapping math and visible output use the same
  display-safe text.

This makes the contract explicit instead of relying on each renderer to remember
its own control-character handling.

## Implementation

- Added `src/tui/display_sanitize.rs` as the central display sanitizer.
- Sanitized streaming `AgentMarkdownStreamState` deltas before feeding the
  markdown stream collector.
- Sanitized committed `Agent`, `User`, `System`, and generic formatted message
  inputs before markdown rendering.
- Sanitized live progress event writes before they reach active-turn rendering.
- Reused the same sanitizer for terminal-event output lines.
- Sanitized inline history `Line` segments before physical row calculation and
  terminal printing.

## Validation

- `cargo fmt --check`
- `cargo test sanitize -- --nocapture`
- `cargo check`

The test set covers:

- plain sanitizer behavior;
- styled `Line` sanitization without losing style;
- terminal event previews;
- active streaming agent display;
- active live progress event display;
- committed agent markdown display.

## Follow-up

No new backlog item is needed. The remaining broader TUI transcript polish stays
under the existing `TUI / Transcript` todo section.
