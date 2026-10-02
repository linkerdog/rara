# Durable Thread Goal Mutations

## Summary

Issue [#930](https://github.com/linkerdog/rara/issues/930) identified that only
TUI goal creation wrote SQLite. Tool creation, lifecycle changes, usage,
budget limiting, and clear remained memory-only. Thread restore also reset
creation time and narrowed stored counters with unchecked casts.

## Reference Patterns And Plan

- Codex `codex-rs/ext/goal/src/api.rs` serializes external goal mutations,
  writes the thread-goal record, and deletes it on clear. Its accounting permit
  in `accounting.rs` spans the snapshot and successful persistent usage write.
- Claude Code `src/utils/sessionStorage.ts::getTranscriptPathForSession`
  documents why active session ID and project storage root must stay aligned
  across resume/branch. This is a session-binding pattern, not a claim that
  Claude Code implements the same goal API.
- Adapt these patterns to one private runtime goal store. Keep the existing
  database schema, tool response fields, lifecycle policy, and input-token
  accounting algorithm. Make persistence errors stop publication/continuation,
  and prove status, usage, time, and deletion through fresh-app restoration.

## Implementation And Trade-offs

- `runtime_goals.rs` owns goal types and a session-scoped store. A mutex spans
  candidate mutation, SQLite write, and snapshot publication. Writers no longer
  expose an unrestricted mutable handle to production callers.
- Commands, model tools, and runtime turn accounting use the same writer.
  TUI state is a presentation snapshot; commands do not write SQLite directly.
- Runtime assembly binds the goal store to the actual agent thread and data
  root. Persistence-disabled embedded profiles stay explicitly in-memory.
  Runtime rebuild inherits the old binding and goal, not a newly generated ID.
- A checked load distinguishes an absent row from an unreadable one and
  deserializes original creation time, statuses, and numeric bounds. A failed
  thread restore retains the previous valid binding. An unavailable initial
  binding cannot silently accept an in-memory goal.
- Clear deletes the existing row. Mutations preserve their goal's creation
  timestamp; replacement writes the new goal's timestamp. No schema migration
  or provider/tool protocol change is required.
- A failed accounting write returns no continuation, keeps the runtime agent,
  and displays a failure notice. A failed command/tool write retains the last
  committed goal snapshot rather than reporting success.
- Existing state-database schema and rollout-migration helpers, and existing
  goal/session-continuity tests, are mechanically split into private modules
  so every touched Rust source file stays below 1000 lines.
- Add the existing `log` package to `rara-state` so legacy optional loads
  surface diagnostics. Cargo and Bazel module locks are refreshed without
  changing dependency versions or Bazel configuration.

## Validation

Behavioral RED: `tool_created_goal_survives_a_fresh_app` fails against the prior
implementation because the tool-created goal has no durable row. This is a
runtime assertion failure, not a compilation failure.

Focused checks:

```bash
cargo test --locked --lib goal
cargo test --locked --lib tui::
cargo test --locked --lib runtime_context
cargo test --locked --lib --quiet
cargo test --locked -p rara-state
cargo check --locked
cargo clippy --locked --all-targets --no-deps -- -D warnings
cargo fmt --all -- --check
git diff --check
bazel mod deps
```

The goal-filtered suite covers tool create/update, command create/pause/resume/
clear, all lifecycle statuses, budget accounting, original timestamps, corrupt
rows, replacement, rebuild binding, and write failures. The actual thread
restore test checks all five statuses and a cleared goal in fresh TUI apps.
SQLite failure triggers prove failed create/delete/accounting writes do not
publish new snapshots. A task-completion test also proves that accounting
failure keeps the returned agent and does not launch a continuation.

The root library suite reports 1531 passing tests and one ignored fixture;
the state-database suite reports 10 passing tests. Strict Clippy completes
without source warnings. The macOS debug linker still reports its existing
large `__eh_frame` compact-unwind limitation; no linker settings are changed.
Remote exact-head CI remains a separate gate after publication.

## Remaining Work

- Issue #931 owns restored-goal auto-continuation and richer goal interaction;
  this change restores state but does not schedule a resumed goal automatically.
- These are SQLite and fresh-app/thread restoration checks, not a manual
  quit/restart PTY acceptance run. Multi-process goal synchronization and a
  different token-accounting policy are outside this change.
