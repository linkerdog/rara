# Portable LLM Contracts

## Summary

Phase 1 of [#871](https://github.com/linkerdog/rara/issues/871) consolidates the
provider-neutral completion contract in `rara-core`. The root API remains a
compatibility re-export. The [feature spec](../features/portable-llm-contracts.md)
records the canonical paths and compatibility limits.

## Decisions

- Align `TokenUsage` and `LlmResponse` with the shape used by the root runtime;
  remove unused duplicate fields and convenience methods from the old core API.
- Move the backend trait, turn metadata, summary prefix, budgets, modes, cache
  profile, stream events, summary strategy, and request fingerprint into core.
- Keep the provider adapters, native transports, and `MockLlm` in the root.
- Add the observability dependency to both Cargo and Bazel. Its task handles
  are needed by turn metadata; this direction is acyclic.
- Gate standalone core tests in test CI and browser-target compilation in the
  existing build job. Root provider/embedded tests continue to validate callers
  using compatibility paths.

## Validation

- `cargo test --locked -p rara-core`: three tests cover the root response JSON
  shape, unknown usage/cache defaults, and summary-prefix history preservation.
- `cargo check --locked --target wasm32-unknown-unknown -p rara-core`: compile
  check for the browser target without feature flags.
- `cargo clippy --locked -p rara-core --all-targets --no-deps -- -D warnings`.
- `cargo fmt --all -- --check` and `git diff --check`.
- Required full build/test/Clippy/format and Bazel CI are checked on the pushed
  PR head; final receipts are recorded on the PR rather than predicted here.

The initial reviewed head had an independently reproduced LSP startup race in
test CI. Its bounded exit-observation repair is recorded in a separate
[checkpoint](2026-09-14-lsp-startup-exit-race.md).

## Follow-Ups

Browser compilation does not prove browser execution. Native `Instant`-based
accounting and Send backend futures need a browser-host design alongside future
provider/transport extraction. Those later phases remain under #871 and
[the active backlog](../todo.md#portable-provider-boundary).
