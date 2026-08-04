# Compaction Timeline Cell

## What changed

The TUI now renders typed compaction transcript entries with a dedicated
timeline cell. The cell shows the compaction number, before/after token counts,
estimated tokens saved, summary, and recent file count.

## Why

OpenCode presents compaction as a first-class session item. Rendering it as a
generic transcript message made the context boundary hard to scan and coupled
the presentation to role strings. The dedicated cell consumes the structured
payload produced by the runtime projection.

## Trade-offs

The cell is additive and supports both active and committed turns. Legacy
role/message persistence remains unchanged. Tool identity correlation and the
remaining role-based renderer paths are intentionally separate follow-ups.

## Verification

- `cargo fmt --all`
- `cargo test --bin rara committed_turn_renders_compaction_as_a_dedicated_cell --no-fail-fast`
- `cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings`
