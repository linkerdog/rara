# Agent Trace Main Integration

## Scope And References

Preserve opt-in session-local JSONL tracing across the shared model execution
refactor. Keep both trace and tool-effect modules, both CLI overrides, and the
current inference lease; the trace schema and prompt construction are unchanged.

Inspected local Codex `f959e7fc9832dfa0ebfb6542ab1bbf829638ac24`,
`codex-rs/otel/src/events/session_telemetry.rs::record_api_request`, and Claude Code
`4b9d30f7953273e567a18eb819f4eddd45fcc877`, `src/services/api/logging.ts::logAPIError`
and its call in `src/services/api/claude.ts`. Both record duration and outcome at
request completion, including errors. Adapt that ownership pattern while keeping
this trace's stricter content-free contract: no provider error message is stored.

## Plan

1. Resolve module, CLI, dependency, and feature-index conflicts additively.
2. Retain `begin_inference_turn` and the shared model executor. Move completion
   observations to `NativeModelPolicy::observe_response`, which sees both results.
3. Cover successful cache receipts and failed-provider completion, including
   exclusion of prompts, responses, and provider error text.
4. Validate trace, config, CLI, and agent integration with strict Clippy. Inspect
   the older PR's Bazel declaration gap separately under the repository's explicit
   build-configuration approval rule.

## Implementation And Regression

Model completion now belongs to the native model policy. A focused failing-backend
regression first reproduced the missing failure record after a success-only hook
migration (zero model records instead of one). The error path now records failure
before returning, without serializing the provider error. The shared fixture also
retains successful cache-receipt and content-exclusion assertions.

## Validation

- `cargo test --locked --lib agent::tests::agent_trace -- --nocapture`: 2 passed.
- `cargo test --locked -p rara-agent -p rara-agent-trace`: 35 shared-agent and
  2 trace tests passed.
- `cargo test --locked -p rara-config agent_trace`: 1 passed.
- `cargo test --locked --lib app_cli::tests -- --nocapture`: 32 passed.
- `cargo clippy --locked --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --check` and `git diff --check`: passed.

## Follow-Ups

The original trace crate lacks its Bazel library/test targets and the root
binary dependency. A minimal patch is prepared; application awaits explicit
approval under the repository's Bazel configuration rule. Cargo validation
does not cover that missing build declaration.
