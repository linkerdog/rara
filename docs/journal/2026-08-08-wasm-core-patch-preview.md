# WASM Core Patch Preview

## Summary

Added `rara-wasm-core` as the first pure logic crate for future browser and
worker integrations. The initial API exposes structured patch preview over a
virtual file set and returns a serializable virtual delta aligned with the
native patch application delta.

## Background

The full RARA runtime depends on native capabilities such as terminal control,
process execution, local filesystem mutation, SQLite persistence, MCP/LSP
processes, and local model runtimes. Those capabilities should remain in the
native runtime. The WASM boundary should carry deterministic reducers,
protocol helpers, validation, and preview logic that can be tested without host
side effects.

## Scope

- Added `crates/rara-wasm-core`.
- Reused `rara-apply-patch` for patch parsing, hunk validation, and text
  application.
- Added `PatchPreviewRequest`, `PatchPreview`, `VirtualPatchDelta`, and
  serializable virtual patch change types.
- Added focused unit tests for successful multi-operation preview and missing
  update target failure.
- Documented the WASM core boundary in `docs/features/wasm-core.md`.

## Key Decisions

- The crate does not depend on `wasm-bindgen` yet. The Rust API is the stable
  internal boundary; JavaScript bindings can wrap it when a concrete browser
  consumer exists.
- Patch preview is dry-run only. Actual filesystem mutation remains native and
  continues to use the tool-layer `AppliedPatchDelta`.
- Successful virtual deltas are exact because all readable state is supplied by
  the request payload.

## Validation

- `cargo test -p rara-wasm-core`
- `cargo check -p rara-wasm-core`
- `bazel test //crates/rara-wasm-core:rara_wasm_core_tests`
- `git diff --check`

## Follow-Ups

- Add CI coverage for `wasm32-unknown-unknown` after the target is installed
  in CI.
- Add a JavaScript binding package around `rara-wasm-core` when there is a
  browser client ready to consume it.
