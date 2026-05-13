# TUI Transcript Chronology

## What Changed

Committed turn rendering now treats the transcript entry list as the ordering
source of truth. The renderer walks entries once and emits user messages,
progress blocks, tool calls, terminal cells, interaction completions, system
messages, and agent messages in that recorded order.

Adjacent progress entries with the same role are still merged into one block so
`Thinking`, `Exploring`, `Planning`, and `Running` sections do not become noisy
when the runtime emits several consecutive updates.

Tool-message fallback cells keep a Codex-style small main-view budget. They
show enough context for scanning while leaving full terminal/output fidelity to
dedicated terminal cells and future transcript-detail surfaces.
When a fallback tool message exceeds that budget, RARA now favors the newest
tail lines instead of keeping the first line, matching the Codex TUI behavior
where long command output keeps the recent end of the stream visible in the
main chat view.

## Why

The previous committed renderer optimized for compact semantic groups. That
made completed turns easier to scan in simple cases, but it also reordered
mixed turns: progress blocks could move ahead of earlier assistant messages,
completion records were sorted by interaction kind, and tool-heavy turns could
collapse to only a final assistant message. The TUI needs a single chronology
across exploration, thinking, assistant output, user input, and tool activity.

## Validation

- `cargo test --bin rara committed_turn_cell -- --nocapture`
- `cargo test --bin rara active_turn_cell -- --nocapture`

## Remaining Work

- Extend the same chronology contract to future structured tool renderers for
  write/update diffs and high-fidelity bash streaming.
