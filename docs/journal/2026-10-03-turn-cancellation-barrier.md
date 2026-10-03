# Turn Cancellation Barrier

## Summary

Issue [#922](https://github.com/linkerdog/rara/issues/922) identifies early TUI
terminal publication and missing late-event fencing. The implementation starts
from `d72827e1f6d0cd1df4e7ca5feeae2dd58373296e` with no unrelated changes.

## Design And Scope

1. Capture early cancellation and stale-event failures in focused regressions.
2. Serialize typed stop acceptance with task return, stamp query events with
   session/turn identity, and publish exactly one execution-owned terminal event.
3. Fence controller projections before deduplication/barrier observation, then
   validate focused, TUI, root, lint, and default Bazel checks.

Each stage exits when its focused contract is proven. Protocol payloads, database
schema, provider cancellation mechanics, and native RuntimeSession ownership
remain unchanged. The compatibility task bridge follows the canonical
[runtime session](../features/runtime-session.md#turn-stop-control) contract.

## Reference Patterns

- Codex `ea2046f36d5ee12d39c8e168fc3e5129301afa2b`:
  `chatwidget/safety_buffering.rs` rejects a noncurrent turn ID;
  `chatwidget/protocol.rs` uses turn/item identity to repair completion without
  duplicating a completed message. Adapt explicit identity, not UI internals.
- Claude Code `4b9d30f7953273e567a18eb819f4eddd45fcc877`:
  `Query.ts` drains remaining tool results on abort before returning;
  `screens/REPL.tsx` guards asynchronous finalization with the query generation.
  Adapt execution-return ownership and stale-finalizer fencing.

## Implementation

- Added session-scoped `QueryTaskControl` with a shared mutex state and typed
  first stop kind. Stop acceptance and execution return are serialized;
  completed or failed join handles reject requests even before UI completion.
- Query callbacks and execution-owned terminal events carry the same existing
  session/turn fields. The original dispatcher remains wired. Its final stop is
  replaced at task return, and its final error is held for the task outcome;
  intermediate diagnostics are flushed before the next callback, not discarded.
  Nonrecoverable diagnostics alone do not satisfy the TUI completion barrier.
- Added controller fencing before cursor mutation and terminal observation.
  Foreign sessions, mismatched/closed turn IDs, and unscoped query output or
  completion cannot reopen streams or finish queued work. Unscoped runtime
  catalog events remain valid and cannot lower the sequence watermark.
- Closed turn IDs are retained within the presentation session and reset only
  when its authoritative session identity changes. Join failure closes identity
  even before the controller consumes TurnStarted. Runtime errors still surface.
- Review tasks receive the same identity/terminal ownership while retaining raw
  subscriber delivery and their existing lack of a cancellation token.
- Split the touched 1173-line task test file into a 901-line parent and a
  282-line plan/control child; existing assertions remain intact. New query
  lifecycle tests live separately. No touched Rust file exceeds 1000 lines.

## Validation

Four genuine RED checks on the frozen base reproduced early terminal publication,
post-terminal text/tool admission, prior-turn terminal/tail admission, and
foreign-session cursor pollution. They now pass. Fifteen new tests plus the
updated input regression cover typed stop races (64 concurrent stop/return
interleavings), both cancel/interrupt completion orders, real task trailing
deltas, queued-query isolation, cancellation after return, diagnostic
preservation, join failure, and identity/cursor fencing. Existing render fixtures now include TurnStarted;
their rendering and frame-count assertions are unchanged. No snapshots changed.

Check results:

- `cargo test --locked --lib tui:: --quiet`: 776 passed.
- `cargo test --locked --lib --quiet`: 1641 passed, 1 ignored (the explicit
  paid-call cache trial), no failures.
- `cargo clippy --locked --all-targets -- -D warnings`: passed.
- `cargo fmt -- --check` and `git diff --check`: passed.
- `bazel test //:rara_unit_tests --test_output=errors`: passed with the default
  configuration, invocation `9e534367-e9b5-4e91-8658-feb9656e41eb`;
  test log confirms 1641 passed and the same paid-call fixture ignored.

The first full Bazel attempt exposed a new fixture asserting rejection of
unscoped catalog events after join failure. The fixture now checks closed-query
events only, preserving global runtime updates. A separate fixture correction
removed a second poll of an already joined handle. These fixture failures are
not counted as RED evidence. The existing macOS debug-linker compact-unwind
diagnostic remains; strict Rust/Clippy checks introduce no new warnings.

## Follow-Ups

Exact-head remote CI, review, merge, and real-terminal acceptance remain separate
delivery gates. This checkpoint does not complete the other TUI quality issues.

## Review Integration Checkpoint

Merged parent `6b57389db33c98e1fce1f1ce09adb80e136130c8` without rewriting the
stack. The merge retains main's goal-turn accounting receipt and the terminal
restoration and paste-order fixes. Dependency manifests and generated locks are
unchanged relative to the parent.

The review identified three reproducible gaps: stop admission discarded the
original execution error, completed queries fenced out untagged `/compact`
events, and a lost broadcast terminal could strand a completed task. Focused
regressions reproduced all three before the fixes. The native session actor
confirms that the first accepted stop determines terminal status. Codex's
`core/src/tasks/mod.rs` separates abort admission from completion; Claude Code's
`src/query.ts` drains remaining tool results before returning from an abort.
These references support preserving the execution-return boundary rather than
publishing success or cancellation at input time.

The resulting contract and implementation are:

- Preserve the original execution error both as a diagnostic and as the cause
  of the stopped result. The accepted stop still wins over a successful return
  and discards newly raised interactions. A request after execution return
  cannot remove the returned plan approval.
- Release active-turn ownership at the terminal boundary while retaining the
  closed-turn identity fence. A running query still rejects unscoped turn output;
  subsequent maintenance lifecycle events remain visible.
- Retain the fully sequenced event in the existing task channel before making
  its broadcast visible, under the bus publication lock. Before projecting a
  broadcast, drain receipts only through that event's sequence, keeping a future
  receipt pending. Drain the remainder after joining the producer. Both paths
  use the same identity and sequence fence, so replay cannot duplicate output.
  Replaying only at join would be insufficient: a later catalog event could
  otherwise advance the cursor beyond the lost query tail and terminal.
- Finalize recovered partial output when a query task panics, mark the task
  failed, and return the join error. The existing panic regression caught the
  previously hidden live-stream cleanup gap once recovery made the tail visible.

Validation for this checkpoint:

- `cargo test --offline --locked --lib tui:: -- --nocapture`: 826 passed,
  1 ignored, no failures. Coverage includes actual broadcast lag followed by a
  later catalog sequence, a wholly dropped query projection, replay cutoff at
  the current broadcast, both approval/stop orders, real `/compact`, error cause
  preservation, task panic, and the existing cancel/interrupt interleavings.
- `cargo test --offline --locked --lib runtime_event_bus::tests -- --nocapture`:
  11 passed, including receipt-before-broadcast visibility.
- The first plan-approval fixture accidentally let the initial runtime snapshot
  overwrite Plan mode. Correcting setup order fixed that fixture; its hang is
  not counted as behavioral RED evidence. Scripted task waits now have bounded
  test deadlines. No production cancellation timeout was introduced.

Noncooperative provider cancellation, queued-follow-up policy, and cancelled-goal
usage/state policy remain separate product work. Goal continuation/accounting is
tracked by #931; this change does not silently pause a goal or discard queued
user prompts. Exact-head CI and real-terminal acceptance remain delivery gates.
