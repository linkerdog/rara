# Unicode Display And Editing Boundaries

## Summary

Issue [#924](https://github.com/linkerdog/rara/issues/924) covers terminal-column
truncation, UTF-8-safe session titles, grapheme editing, canonical selection,
and checked goal restoration. The source baseline is
`c33161184e31bd078dc7642b56d928ff712eefb4`, stacked on the display-ingestion work.

## Reference Patterns

- Codex `ea2046f36d5ee12d39c8e168fc3e5129301afa2b`: width and styled-line
  truncation share column measurement; textarea movement uses atomic grapheme
  boundaries. Its additional text-element and Thai-mark policies are not
  adopted by this focused whole-grapheme editor change.
- Claude Code reference checkout `4b9d30f7953273e567a18eb819f4eddd45fcc877`:
  `MeasuredText` exposes grapheme boundaries and offset snapping; truncation
  accumulates column widths without splitting segments. Offset units and NFC
  normalization are not copied into the existing character-offset model.
- Pinned Ratatui 0.30.2 / core 0.1.2: buffer output skips standalone zero-width
  graphemes, but includes halfwidth dakuten/handakuten in cell width. Selection
  must consume the same display projection, preserving visible joined clusters.

## Implementation Plan

1. Diagnostic and title boundaries: capture real width/UTF-8 failures, sanitize
   full styled physical lines, clip all diagnostic chrome and prefixes, and
   abbreviate session titles by whole-grapheme display columns. Exit requires
   narrow width and Unicode matrix assertions while preserving ASCII labels.
2. Editing and selection: keep stored character offsets; share explicit
   floor/ceil/previous/next grapheme boundaries across existing text targets
   and paste bursts. Normalize complete styled rows before selection so
   invisible standalone clusters cannot leak into copy or split visible
   clusters across spans. Exit requires state, rendered-buffer, and drag/copy
   assertions, including clusters joined by insertion or deletion.
3. Goal restore: validate persisted numeric fields without changing the durable
   schema. Invalid snapshots leave the thread usable, clear stale session-local
   goals, retain the stored snapshot, and provide a field-specific notice. Exit
   requires production restore-path coverage and exact-limit decoding checks.

No protocol, package dependency, persistence schema, Bazel configuration, or
history rewrite is part of this plan. Local source checks, exact-head remote
CI, review/merge, and physical-terminal acceptance remain separate gates.

## Implementation Checkpoint

- Shared prefix/suffix column primitives replace scalar/byte truncation in
  diagnostics, startup labels/paths, and session titles. Every diagnostic row
  is normalized and clipped once, including oversized prefixes and chrome.
  Maximum LSP coordinates convert to `u64` before one-based formatting.
- Character-offset storage remains unchanged. Shared editor navigation,
  deletion, insertion, and paste bursts now use whole-grapheme boundaries.
  Deletion repairs boundaries if neighboring regional indicators join;
  insertion/paste snaps past newly completed ZWJ clusters.
- Styled physical rows omit standalone zero-width clusters and repair
  cross-span clusters. Removing an invisible separator triggers final
  re-segmentation, making normalization idempotent. Already valid style-span
  boundaries stay intact, preserving the existing cache equality contract.
- Goal restore uses checked numeric admission, field-specific warning/notices,
  exact-limit acceptance, and non-destructive rejection. Absent/invalid target
  snapshots clear stale session-local goals. Replacing validated state recovers
  a poisoned goal handle; this is not a general panic-site audit.
- The shared startup/path helpers had the same scalar truncation defect and a
  zero-width marker overflow, so they now reuse the column/grapheme boundary.
  ASCII marker styles remain unchanged (`...` for diagnostics; `…` elsewhere).

## Validation

- Ten production-path baseline regressions failed at `c331611`: diagnostic
  overflow, multibyte title panic, scalar navigation/deletion, joiner insertion
  and paste, invisible/cross-style row projection, and overflowing goal restore.
- An additional pre-fix test against unchanged startup/path helpers exposed
  their zero-column ellipsis overflow. Both shared truncation paths now satisfy
  narrow/CJK/ZWJ/combining/zero-width matrices.
- Twenty-five added focused tests cover these boundaries. The existing ASCII
  truncation test moved to the shared styled clipping module without losing
  its assertions. No snapshots were changed.
- `cargo test --quiet`: 1,690 root unit tests passed, one explicit paid-provider
  cache trial remains ignored; both integration suites passed (1 and 8 tests).
- `cargo test --quiet --lib tui::`: all 825 TUI tests passed on the final source.
- `bazel test //:rara_unit_tests --test_output=errors`: 1,690 passed with the same
  paid trial ignored. Invocation `6cafef26-7f38-4c9e-8377-cf1b8f009cb8` used the
  unmodified default configuration and completed successfully.
- `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, and
  `git diff --check`: passed. Touched source files remain below 1,000 lines.
- The existing macOS debug-linker compact-unwind warning appeared in both the
  baseline and final Cargo checks; no new Rust/Clippy warning was introduced.

## Follow-Ups

Exact-head CI/review/merge and physical-terminal/clipboard acceptance are not
established by source tests. Resume-search full cursor editing and an opt-in Vim
mode remain separate work.
