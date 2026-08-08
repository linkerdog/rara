# WASM Core

## Problem

RARA has native runtime capabilities that cannot be compiled directly into a
browser or worker target: PTY management, process execution, local filesystem
mutation, SQLite-backed persistence, MCP/LSP transports, and terminal UI
rendering. Browser clients still need a small, deterministic subset of RARA
logic so they can preview user-visible effects before delegating native work to
the runtime.

## Scope

- Introduce `rara-wasm-core` as a pure Rust crate for browser- and
  worker-oriented logic.
- Expose structured patch preview and virtual patch delta construction.
- Keep the initial API independent of JavaScript binding tools so it remains
  usable from Rust tests and future `wasm-bindgen` adapters.
- Reuse `rara-apply-patch` as the source of truth for parsing and applying the
  structured patch format.

## Non-Goals

- Compiling the full `rara` CLI/TUI binary to WASM.
- Running tools, shell commands, PTYs, MCP servers, LSP processes, SQLite, or
  local model execution in WASM.
- Adding a JavaScript package or generated bindings in this phase.
- Replacing native `apply_patch` execution. The WASM API only previews a
  virtual file set.

## Architecture

`rara-wasm-core` is a leaf crate over pure logic crates. Its first contract is
patch preview:

1. A caller submits a patch string plus a virtual file map.
2. The crate delegates parsing and hunk application to `rara-apply-patch`.
3. The crate returns summary lines, action stats, bounded text preview, and a
   serializable virtual delta.

The virtual delta mirrors the native `AppliedPatchDelta` shape used by the
tool layer, but it is computed without touching the host filesystem. This keeps
browser clients and native tools aligned on the same patch semantics while
preserving the native runtime as the authority for actual mutation.

## Contracts

### `PatchPreviewRequest`

- `patch`: structured RARA patch text.
- `files`: map from path to current file content.

### `PatchPreview`

- `summary`: human-readable operation summaries.
- `stats`: file, hunk, added-line, removed-line, created, deleted, moved, and
  updated counts.
- `preview`: bounded patch text preview and truncation flag.
- `delta`: exact virtual patch delta.

### `VirtualPatchDelta`

- `exact`: always `true` for successful pure previews because every input file
  is supplied by the request.
- `changes`: ordered patch changes.

### `VirtualPatchFileChange`

- `add`: new content plus optional overwritten virtual content.
- `delete`: removed virtual content.
- `update`: original content, optional move target, optional overwritten move
  target content, and new content.

Errors are surfaced as `WasmCoreError::PatchPreview` and preserve the
underlying patch failure message.

## Validation Matrix

- `cargo test -p rara-wasm-core`
  - serializable virtual delta for add, delete, update, and move operations;
  - request payload wrapper behavior;
  - missing update target failure.
- `cargo check -p rara-wasm-core`
  - verifies the new crate independently of native-only dependencies.
- `bazel test //crates/rara-wasm-core:rara_wasm_core_tests`
  - verifies the Bazel target mirrors Cargo wiring.
- Future `wasm32-unknown-unknown` check
  - should be added once CI installs the target and binding requirements are
    settled.

## Open Risks

- The initial crate is WASM-oriented pure Rust, not yet a published JS package.
- The repository still needs CI coverage for `wasm32-unknown-unknown`.
- Future APIs must avoid accidentally depending on native-only crates through
  convenience imports.

## Source Journals

- `docs/journal/2026-08-08-wasm-core-patch-preview.md`
