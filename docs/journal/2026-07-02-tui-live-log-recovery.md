# TUI Live Log Recovery

## Summary

Connected the existing per-session `live.jsonl` transcript log to the TUI
active-turn lifecycle.

## What Changed

- `TuiApp` now writes transcript entries to the live log as they are appended.
- Active-turn rewrites, such as final assistant text replacement, rewrite the
  live log so restart recovery does not restore stale intermediate text.
- Committing a turn to `turns.jsonl` clears the live log so resume only restores
  genuinely incomplete turns.
- Thread resume loads remaining live entries into `active_turn` after committed
  turns are restored.

## Design Notes

Claude Code's resume path treats persisted transcript data as the source of
truth and detects interrupted turns while deserializing. RARA keeps the existing
committed-turn contract intact and uses `live.jsonl` as a small side log for
only the uncommitted active turn.

## Validation

```bash
cargo fmt
cargo test tui::state::tests::active_turn_entries_write_and_clear_live_log -- --nocapture
cargo test tui::session_restore::tests::restore_session_recovers_live_active_turn_entries -- --nocapture
cargo test tui::state::tests -- --nocapture
cargo test tui::session_restore::tests -- --nocapture
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings
git diff --check
```

## Follow-Up

Terminal event payloads are still restored through their text transcript shape,
matching committed turn persistence. Preserving typed terminal payloads across
live recovery would require extending `PersistedTurnEntry`.
