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
