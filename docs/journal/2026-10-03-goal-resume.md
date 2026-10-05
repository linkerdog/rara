# Goal Resume and Local Controls

## Summary

Explicit TUI thread restoration now queues one eligible goal continuation and
waits for idle runtime admission. Cancellation and interruption persist a
separate continuation deferral, including a late stop after execution returned.
The completed turn retains its actual result and successful-turn accounting.

Local goal controls now provide a paused-resume choice, full summary, objective
editor, unfinished-goal replacement confirmation, and elapsed/budget status.
The model-facing goal tool response and completion-authority contract are
unchanged. Goal commands live in a focused runtime module; display and input
state remain separate.

## Scope and Delivery

1. The serialized GoalStore persists private deferral metadata through an
   additive, checked SQLite column. Clear/replacement reset deferral; mutation,
   editing, and backend rebuild preserve it. A new turn clears it durably.
2. Restore and explicit command requests carry a one-use goal revision.
   Consumption rechecks readiness, pending interactions, queued user work,
   overlays, mode, lifecycle, and budget before taking the agent. A new turn or
   thread/goal change invalidates old requests. Pending startup rebuilds retain
   the request. Bootstrap without explicit restore never arms it.
3. Paused choice, summary, edit, replacement confirmation, and status use the
   existing overlay/key routing, shared Unicode wrapping, and semantic colors.
   Editing preserves usage/budget/status, cancellation leaves data unchanged,
   and stale confirmations cannot replace another goal. Busy `/goal pause`
   stops future continuation while the current turn finishes.
4. Continuation after a completed turn also yields to pending questions or
   approvals and gives queued user work precedence over another substantive
   goal turn. Successful usage is still charged. Fractional token budgets that
   round to zero are rejected instead of creating an unrestorable goal.

## References and Decisions

- Codex checkout `f959e7fc9832dfa0ebfb6542ab1bbf829638ac24`:
  `codex-rs/ext/goal/src/runtime.rs` restore/idle hooks,
  `codex-rs/core/src/session/inject.rs` atomic idle admission,
  `codex-rs/tui/src/app/thread_goal_actions.rs` resume choice, and
  `codex-rs/tui/src/chatwidget/goal_menu.rs` summary/editor.
- Claude Code checkout `4b9d30f7953273e567a18eb819f4eddd45fcc877`:
  `src/query.ts` stops continuation on API errors and aborted tools;
  `src/query/stopHooks.ts` honors explicit continuation prevention.
  This is a stop-boundary reference, not an equivalent durable goal feature.
- Adapt these patterns to the existing session GoalStore and TUI command port.
  Preserve write-before-publish and existing successful-turn accounting.
  A restore request needs identity checks both before enqueue and at admission;
  a one-time startup send loses work during rebuild and can execute stale work.
- The additive interruption flag cannot reconstruct interrupts from older
  binaries. Existing lifecycle states remain authoritative on migration.
- `goal-evaluator-loop.md` is historical and now links the canonical contract.

## Validation

Behavioral regression evidence:

- Before the persistence fix, a saved interruption marker loaded as `null`.
- Removing the production idle hook made the real event-loop test capture zero
  commands instead of one. Removing new-turn revision invalidation admitted a
  stale queued goal and took the agent. Both deliberate mutations were restored.
- Before the completion fixes, pending input still returned a continuation,
  queued user work lost priority, and a `0.1` token budget parsed as `Some(0)`.
  Focused tests reproduced all three failures before their fixes.

Verification includes real asynchronous event-loop checks with FakeRuntimeClient
(startup-style restore, latest/picker, fresh bootstrap, not-ready retry,
exactly-once admission, stale requests, permission override, user cancellation),
fresh-database round trips, old-schema migration, injected persistence failures,
and harness rendering/key routing for goal controls and Unicode editing.

- `cargo test --lib tui::`: 951 passed; 3 isolated child fixtures are exercised
  by their parent tests.
- `cargo test --lib goal`: 89 passed.
- `cargo test -p rara-state`: 10 passed; doc tests passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --check` and `git diff --check`: passed.
- Default `bazel test //:rara_unit_tests --test_arg=tui::`: 951 passed and
  3 isolated child fixtures, matching Cargo; 105 seconds total, 13 seconds test.
  Initial dependency loading failed in the existing external `rules_rust`
  cache. Scoped `bazel fetch --force //:rara_unit_tests` restored dependencies
  in 673 seconds; no Bazel configuration, BUILD, or lock files changed.
  The existing gold-linker deprecation warning remains.
- All touched Rust files remain below 1000 lines. Existing snapshots are
  unchanged; goal presentation has focused screen and state assertions.

## Limits

- Migration defaults old interruption flags to false because older binaries
  never recorded user stops. Existing non-active lifecycle statuses stay idle.
- A failed durable stop warns that a restart may continue the goal; the queued
  in-process request is still invalidated. The unreadable-row recovery contract
  remains fail-closed.
- Token usage keeps the existing successful active-turn policy. Cancelled/error
  provider spend is not reconciled as a complete usage ledger.
- These changes target the TUI compatibility runtime; they do not introduce
  external protocol fields, a new completion classifier, or a second goal loop.
