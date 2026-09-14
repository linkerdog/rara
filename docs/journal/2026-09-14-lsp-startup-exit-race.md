# LSP Startup Exit Race

## Problem

The test job for PR #873 failed twice on `1e6d5e16`: the early-exit fixture
expected `ServerExited` with exit code 17 and stderr, but received
`ProtocolError`. The contract extraction did not change LSP code. Inspection
found that a failed initialize write returned before the child supervisor could
publish the exit receipt, then runtime drop cancelled that supervisor.

## Design And References

The local Claude Code `src/services/lsp/LSPClient.ts` handles process exit
separately from stdin errors. Codex's
`codex-rs/rmcp-client/src/executor_process_transport.rs` distinguishes process
exit from completed output draining. Both support keeping process status and
diagnostics under supervision rather than treating the first transport event
as the complete failure.

Adapt that pattern with a bounded startup-only wait: after a retryable protocol
error, retain the runtime for up to 250 milliseconds while waiting for the
existing supervisor receipt. Prefer its typed exit status and stderr. If the
server stays alive or the observer closes, preserve the protocol error.
Initialize timeouts and non-retryable failures bypass this wait. No protocol,
public API, or test expectation changes are needed.

This is an independent CI-blocker repair kept in its own commit alongside the
contract extraction. The stable contract is in
[LSP integration](../features/lsp-integration.md#structured-failures).

## Validation

The deterministic regression supplies a broken-pipe failure before a scheduled
supervisor receipt. It failed before the repair with the same incorrect error
kind as CI. Companion cases cover a live child with no receipt and timeout
classification. The original real-process early-exit test remains unchanged.

- `cargo test --locked -p rara --lib lsp_manager:: -- --nocapture`
- `cargo test --locked -p rara --lib inference_`
- `cargo test --locked -p rara --lib agent::tests::cache_experiment`
- `cargo check --locked -p rara --lib`
- `cargo clippy --locked -p rara --lib --tests --no-deps -- -D warnings`
- `cargo fmt --all -- --check` and `git diff --check`

Final full-suite and Bazel CI receipts are recorded against the pushed PR head.

## Follow-Ups

No additional startup-race work remains in this slice. A server that stays
alive beyond the grace period still receives the original protocol error; the
normal supervisor continues to own process teardown and status.
