# Apply Patch Crate Split

## Summary

RARA now has a standalone `rara-apply-patch` crate for the pure structured
patch parser and text update engine. The user-facing `apply_patch` tool remains
in `rara-tools`, where it owns tool input parsing, stale-read enforcement, file
I/O, and JSON result formatting.

## Background

Codex keeps its patch grammar and application logic in a dedicated
`apply-patch` crate. The useful architectural lesson for RARA is the boundary:
patch parsing and change derivation should be reusable runtime infrastructure,
while the tool layer should stay responsible for session policy and filesystem
side effects.

## Scope

- Added `crates/rara-apply-patch`.
- Moved typed patch operations, update chunks, patch parsing, update-context
  validation, preview truncation, line joining, lenient sequence matching, and
  Unicode normalization into the new crate.
- Added `PatchAction`, `PatchChange`, and `PatchActionStats` so callers can
  derive a typed preview from patch text and a file reader before applying
  filesystem side effects.
- Kept `rara-tools::patch::ApplyPatchTool` as the user-visible tool wrapper.
- Preserved the existing tool schema, stale-read guard, dry-run behavior, and
  JSON result shape.

## Key Decisions

- Do not depend directly on Codex's internal `apply-patch` crate. It is coupled
  to Codex's exec-server and path URI abstractions, while RARA needs a small
  provider-neutral patch engine that can later serve TUI, ACP, Wire, and
  app-server surfaces.
- Keep filesystem writes in `rara-tools` for this step. A later PR can add an
  applied-delta failure contract once this crate boundary is stable.

## Validation

```bash
cargo test -p rara-apply-patch
cargo test -p rara-tools --no-run
RUST_TEST_THREADS=1 target/debug/deps/rara_tools-b2c9b4642b5a0fa3 patch::tests --nocapture
cargo fmt --check
cargo check
cargo clippy -p rara-apply-patch -p rara-tools --all-targets -- -D warnings
git diff --check
```

## Follow-Ups

- Add structured applied-delta failure reporting for partial filesystem writes.
