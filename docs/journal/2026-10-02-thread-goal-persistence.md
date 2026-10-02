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
- Follow-up references: Codex `ext/goal/src/runtime.rs::restore_after_resume`
  checks the stored goal before publishing accounting state, and its
  `extension.rs::on_thread_resume` reports restoration failures without aborting
  the thread callback. Claude Code `conversationRecovery.ts` reports and
  rethrows deserialization errors. Keep required thread reads transactional;
  isolate optional goal binding during bootstrap and resume with a visible
  diagnostic instead of rejecting an otherwise readable thread.
- Adapt these patterns to one private runtime goal store. Keep the existing
  database schema, tool response fields, lifecycle policy, and input-token
  usage metric. Make persistence errors stop publication/continuation,
  and prove status, usage, time, and deletion through fresh-app restoration.
- Further review: Codex's goal runtime treats active and budget-limited states
  separately instead of restarting active work from an exhausted budget.
  Apply that distinction before local resume dispatch. Keep successful query
  snapshot publication independent from its optional goal accounting write;
  Claude Code's recovery path likewise keeps restored conversation state
  explicit rather than inventing a goal-accounting API.

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
  deserializes original creation time, statuses, and numeric bounds. Failed
  required thread reads retain the previous valid binding. An unavailable
  optional goal binding cannot silently accept an in-memory goal.
- Clear deletes the existing row. Mutations preserve their goal's creation
  timestamp; replacement writes the new goal's timestamp. No schema migration
  or provider/tool protocol change is required.
- A failed accounting write returns no continuation, keeps the runtime agent,
  publishes that agent's completed runtime snapshot/session metadata, and
  displays a failure notice. A failed command/tool write retains the last
  committed goal snapshot rather than reporting success.
- Resume checks persisted usage before starting a turn. Paused/blocked goals
  at or beyond budget enter durable `BudgetLimited` and use only the existing
  wrap-up prompt; goals below budget retain normal continuation. A failed
  status write sends no query. This does not change which turns are charged.
- Restoration follow-up stages thread, todo, and runtime JSON reads before
  rebinding the goal. Required read failures preserve agent history, session
  ID, presentation, and the previous durable goal binding. Bootstrap and resume
  log a warning and disable goal persistence when optional goal binding fails;
  resume still publishes the requested thread with an empty goal snapshot.
  Neither path rewrites corrupt records or accepts memory-only replacements.
  A later valid restore re-enables durable writes for its own thread.
  Empty/whitespace objectives and zero budgets are rejected before restoration
  publication. `/goal` errors are contained at the command boundary, with a
  visible notice and no event-loop exit or failed-write continuation.
- Explicit and generated bootstrap IDs match the assembled agent's durable
  binding. ACP supplies its session ID, exec uses the bootstrap's ID, and
  subagents allocate their own IDs in `agent_control`/`agent_runtime` without
  traversing this bootstrap path. Keep the TUI type re-exports: presentation
  consumers still use them.
- Review follow-up: Codex captures the active goal identity and token baseline
  at turn start. Adopt that boundary so a successful terminal or paused goal
  turn still persists usage; only the resulting status decides continuation.
  Ephemeral membership survives runtime rebuild, but clear, replacement, and
  thread restoration invalidate it. It cannot charge an identical replacement
  created in the same second. The baseline comes from the actual runtime agent,
  not the presentation snapshot. Plan-only and rejected-plan tasks do not
  participate; approving a plan captures the ensuing implementation query.
  Errored/cancelled turns and budget-limited wrap-up turns are not charged;
  this is successful-active-query accounting, not a full usage ledger.
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

Review RED: `terminal_goal_turn_persists_final_usage_without_continuing`
restores a completed goal with 90 tokens and two turns instead of the expected
105 tokens and three turns after a successful query. This regression exercises
the update tool, task completion, and fresh-app restoration boundary.

Restoration review REDs: a corrupt todo changes the agent ID to the target
thread before failing; a corrupt goal aborts bootstrap; an empty objective is
published as active; and an injected SQLite pause write escapes the local
command loop. These are behavioral assertion failures. The corrected cases
also cover runtime JSON, rejected goal restore, whitespace objectives, zero
budgets, failed clear/create, unchanged durable rows, and actual agent-ID
binding for both explicit and generated IDs.

Resume review RED: an unknown goal status propagates out of
`restore_thread_by_id` instead of restoring the readable target thread. The
regression covers explicit and latest-thread restore with unknown status,
empty objective, and zero budget. It verifies the target thread/history are
published, the prior goal snapshot is removed, warnings remain visible, goal
writes fail closed, both durable rows are unchanged, and a valid subsequent
restore recovers goal persistence. Required todo/runtime failure tests retain
their original rollback contract.

Further review REDs: resuming a paused 10/10-token goal publishes `Pursuing`
instead of `BudgetLimited`, and injected accounting failure leaves input-token
snapshot at 0 instead of the returned agent's 25. The corrected cases exercise
paused/blocked statuses below/at/above budget, exact normal/wrap-up prompt
selection (excluding elapsed-second formatting), durable status round trips,
failed status writes with no dispatch, and completed query token/plan/prompt/
permission snapshot plus persisted session metadata. The last goal snapshot
remains committed and no automatic continuation starts on accounting failure.

Focused checks:

```bash
cargo test --locked --lib goal
cargo test --locked --lib tui::
cargo test --locked --lib runtime_context
cargo test --locked --lib continuity_tests -- --nocapture
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

The review follow-up's focused continuity suite reports 12 passing tests,
including complete/blocked durable final-turn usage, paused status retention,
same-second replacement, thread changes, rebuild membership, and actual query
start with execute/review/plan/approve/refine/reject modes. The root library
suite reports 1536 passing tests and one ignored fixture;
the state-database suite reports 10 passing tests. Strict Clippy completes
without source warnings. The macOS debug linker still reports its existing
large `__eh_frame` compact-unwind limitation; no linker settings are changed.
Remote exact-head CI remains a separate gate after publication.

The restoration follow-up's goal-filtered suite reports 59 passing tests;
the runtime-context suite reports 22 passing tests, and the root library suite
reports 1540 passing tests with one ignored fixture. Source-size limits remain
preserved by a private bootstrap goal-binding module.

The resume follow-up reports nine passing thread-restoration tests, 60 passing
goal-filtered tests, and 1541 passing library tests with one ignored fixture.
`cargo check`, strict all-target Clippy, formatting, and diff checks pass.
The goal persistence test-module declaration is moved to the end of its owning
command module. No new logic is added to the 993-line runtime-context assembly;
the touched session-restore source remains below 1000 lines.

The budget/snapshot follow-up reports 62 passing goal-filtered tests, twelve
passing continuity tests, and 1543 passing library tests with one ignored
fixture. Compilation, strict all-target Clippy, formatting, and diff checks
pass. The existing macOS compact-unwind linker warning remains unchanged.

## Remaining Work

- Issue #931 owns restored-goal auto-continuation and richer goal interaction;
  this change restores state but does not schedule a resumed goal automatically.
- These are SQLite and fresh-app/thread restoration checks, not a manual
  quit/restart PTY acceptance run. Multi-process goal synchronization and a
  different token-accounting policy are outside this change.
- A resume continuation failure followed by a failed durable rollback can
  still report the rollback error instead of the original startup error.
  This existing diagnostic limitation does not launch a continuation and is
  outside the optional restoration correction.
