# CI Test Synchronization

## Scope

Resolve the two test-fixture races in [#972](https://github.com/linkerdog/rara/issues/972).
Production behavior and the assertions about plan mode, agent ownership, LSP
startup sharing, and document notification counts remain unchanged.

## References And Decisions

Codex `f959e7fc` (`codex-rs/tui/src/app/tests.rs`) awaits matching events and
oneshot lifecycle acknowledgements, with timeouts as failure guards. Claude
Code `4b9d30f` (`src/services/lsp/manager.ts`) awaits the shared initialization
promise. The fixtures now use that completion-based boundary:

1. Reuse the plan completion helper across task tests. Await each task handle,
   dispatch its completion through the production adapter, and continue until
   idle. Retain the existing five-second deadlock guard.
2. Wait for the complete LSP transcript predicate, including both expected
   `didChange` notifications, before asserting counts. File polling only
   observes readiness; elapsed time does not establish success.
3. Exercise the partial-write interleaving explicitly: one notification keeps
   the waiter pending, and the second allows it to finish.

## Validation

- The old transcript helper fails the new partial-notification test immediately
  at the pending assertion after only one notification. No timing assumption or
  production mutation is needed to reproduce the failure.
- `cargo test --locked --offline --lib lsp_manager::`: 15 passed, including the
  regression and the original concurrent-startup/document-notification test.
- `cargo test --locked --offline --lib tui::runtime::tasks::tests`: 42 passed,
  including the plain-answer plan test and all existing completion-helper users.
- `cargo clippy --locked --offline --all-targets --no-deps -- -D warnings`,
  `cargo fmt --all`, and `git diff --check` pass.
- These test modules remain in the existing Cargo and Bazel root test targets.
  Exact-head remote CI remains a separate delivery gate.

## Follow-Ups

No production contract or feature-spec changes are required. This checkpoint
does not change the global CI retry policy or claim to eliminate unrelated
timing-dependent tests.
