# Portable LLM Contracts

## Problem

Providers and embedding hosts need one completion contract without depending on
the root application. Duplicated core and root response types had diverged even
though only the root shape was consumed by the runtime.

## Scope

- One canonical message, response, usage, and backend contract in `rara-core`.
- Compatibility re-exports through the existing root paths.
- Native tests and `wasm32-unknown-unknown` compilation without feature flags.

## Non-Goals

Provider extraction, browser HTTP/SSE execution, WASI, and a portable agent loop
are later phases of [issue #871](https://github.com/linkerdog/rara/issues/871).
This phase does not change prompt order, cancellation, accounting, or defaults.

## Architecture

`rara_core::llm::types` owns `Message`, `ContentBlock`, `LlmResponse`, and
`TokenUsage`. `llm::backend` owns `LlmBackend`, `LlmTurnMetadata`, and
`SummaryPrefix`. `llm::contracts` owns budgets, modes, stream events, cache
capabilities, summary strategy, and `ModelRequestFingerprint`.

The root `llm` and `model_observation` paths re-export those same types rather
than wrapping them. Provider implementations and `MockLlm` remain in the root.
The core depends on `rara-observability` for task-owned attempt handles; that
dependency has no reverse edge or transport/async-runtime dependency. Cargo and
Bazel both declare it explicitly.

## Contracts

- The canonical response preserves the previously used root wire shape:
  `content`, optional `stop_reason`, and optional `usage`. Usage contains
  `input_tokens`, `output_tokens`, `cache_hit_tokens`, and `cache_miss_tokens`;
  absent cache counters deserialize as zero. Missing usage stays unknown.
- The old, unused core-only response/usage fields and convenience methods are
  removed. Direct consumers of that former core API must migrate; compatibility
  is guaranteed for the root API, not the discarded duplicate definition.
- Backend calls preserve message/tool order and the existing default fallback
  dispatch. Cancellation and task accounting remain explicit turn metadata.
- A summary prefix separates only the generated first system message. System
  messages within history remain present exactly once; shortened, shifted, or
  changed histories cannot claim prefix reuse.
- Fingerprints contain hashes and counts only. Different hash scopes cannot
  share a reported prefix.

## Validation Matrix

| Boundary | Check |
|---|---|
| Root response wire shape | Core serialization round trips with cache counters and absent usage |
| Summary prefix | Core test retains leading history system messages and rejects shortened history |
| Root compatibility | Existing provider, accounting, cache-experiment, and embedded-runtime tests |
| Standalone core | `cargo test --locked -p rara-core` and core Bazel target |
| Browser target | `cargo check --locked --target wasm32-unknown-unknown -p rara-core` in build CI |
| Workspace integration | Required build, test, formatting, and Clippy checks |

## Operational Notes And Open Risks

Compilation on a browser target is not browser execution proof. Accounting
still uses `std::time::Instant`, and `async_trait` retains Send futures. A future
browser transport must address the clock and non-Send fetch futures before
claiming end-to-end support. Runtime mode and summary strategy defaults are
unchanged by this extraction.

## Source Journals

- [2026-09-14-portable-llm-contracts](../journal/2026-09-14-portable-llm-contracts.md)
