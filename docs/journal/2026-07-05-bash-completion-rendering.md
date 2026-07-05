# Bash Completion Rendering

## Summary

Active TUI turns now render bash completion as a compact status line instead of
showing a raw `bash finished with exit code 0` tool-result sentence at the
bottom of the transcript.

## Background

While the model was streaming thinking text, the active turn renderer could
append the latest successful bash result as a fallback message. This made the
UI look like thinking had stalled behind a stale `bash finished with exit code
0` line.

## Scope

- Suppress the latest tool-result fallback while live thinking or live events
  are already visible.
- Render bash completion fallbacks as `✓ bash` for success and `✗ bash · exit
  N` for failure.
- Keep raw tool-result text unchanged for model context and persistence.

## Validation

```bash
cargo test --locked active_turn_cell_hides_successful_bash_result_while_thinking
cargo test --locked active_turn_cell_renders_bash_completion_as_status_line
cargo test --locked tui::render::cells::tests
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets -- -D warnings
```
