# Session Error Recovery

## Summary

Issue #974 identified operation failures that escaped the TUI event loop:
thread resume, interrupted rollout appends, task join failures, credential
reads, and configuration persistence after backend rebuild. These failures now
leave a visible diagnostic and a usable session or recovery path.

## Reference Review And Plan

Reviewed Codex `codex-rs/rollout/src/recorder.rs::load_rollout_items` and
`codex-rs/tui/src/session_resume.rs` at `f959e7fc`, plus Claude Code
`src/utils/json.ts::parseJSONL` and `src/screens/ResumeConversation.tsx` at
`4b9d30f7`. Both rollout readers recover individual records. This implementation
uses a narrower rule: only an unterminated final record with unexpected EOF is
recoverable; malformed complete records and middle corruption remain errors.

The implementation stages were:

1. Define the recovery contract and reproduce truncated-tail, separator, and
   diagnostic failures at the persistence boundary.
2. Handle resume and credential errors at their operation boundaries, preserving
   current state and the active picker; exercise the dispatch through a fake
   runtime port and startup resume through the shared startup adapter.
3. Recover task completion and config-save failures, then verify retained input
   reaches a fresh query after rebuilding. Keep terminal/transport failures on
   their existing error paths.

## Key Decisions

- Rollout reads leave the original bytes intact and warn with a file and line.
  Before appending to a truncated log, save and sync the original bytes in a
  sibling `events.jsonl.recovery-*` file, then remove only the partial tail.
  Complete records without a newline receive a separator. Advisory file locks
  coordinate readers, appenders, and repair across cooperating processes.
- Required resume reads still precede changes to history, session identity, and
  goal binding. An absent runtime/storage prerequisite now produces an error
  rather than claiming successful restoration.
- A task panic ends its turn, preserves produced text and queued input, and
  removes decisions owned by the lost agent. The existing missing-agent path
  rebuilds on the next submitted prompt. Panicked in-memory agent state is not
  reused; the UI transcript survives and durable threads remain resumable.
- Configuration persistence happens after replacement installation. A failed
  save reports that settings apply only to the current session; it cannot
  discard the valid replacement or the merged history.
- Runtime assembly remains in the existing runtime task path. No new public API,
  storage schema, event shape, dependency, or TUI-owned extension discovery is
  introduced.

## Validation

- Regression checks first reproduced all three persistence failures and both
  task/config completion failures against the previous implementation.
- `cargo test -p rara-persistence`
- `cargo test -p rara --lib tui::`
- `cargo test -p rara --lib tui::session_restore::recovery_tests`
- `cargo clippy -p rara -p rara-persistence --all-targets -- -D warnings`
- `cargo fmt --all`
- `bazel test --test_output=errors //crates/rara-persistence:rara_persistence_tests`

Validation results are recorded in the PR. Local Bazel remains subject to the
existing missing external-cache packages; the default CI check is the merge
gate when local analysis cannot start.

## Follow-Ups

No additional implementation work is deferred from #974. Recovery does not
salvage arbitrary middle corruption or reuse partially unwound runtime objects.
Other issue-list entries and PR acceptance remain tracked separately.
