# Per-File Patch Previews

## Summary

Issue [#985](https://github.com/linkerdog/rara/issues/985) requires all changed
files, operations, counts, and per-file omission markers to survive preview
truncation. The interaction contract is [DIFF-01 through DIFF-03](../interaction/patch-previews.md).

## References And Plan

Inspected local Codex `f959e7fc9832dfa0ebfb6542ab1bbf829638ac24`,
`codex-rs/tui/src/diff_render.rs`: file changes own operation/path/count data,
and hunk rendering is separate from per-file presentation. Inspected Claude Code
`4b9d30f7953273e567a18eb819f4eddd45fcc877`,
`src/components/StructuredDiffList.tsx` and `StructuredDiff.tsx`: each file's
hunks render independently, with explicit separators and width-safe gutters.
Adapt those boundaries while retaining finite preview budgets.

The initial renderer investigation also found the upstream global 120-line cap,
which loses later files before the TUI's 80-line cap. The implementation plan is:

1. Preserve every file in the shared producer, bound content per file, and carry
   complete counts/omissions through existing preview text. Prove the upstream
   regression with a large first file and later delete/move/add operations.
2. Emit the full inventory before per-file hunks. Compose producer and renderer
   omissions, preserve unknown deletion counts, and use shared wrapping.
3. Verify pure/browser previews and actual native dry-run output through runtime
   formatting and transcript rendering, then review snapshots and lint gates.

No new serialized fields, dependencies, database changes, or filesystem reads
are required. Validated counts come from already-built patch changes. Existing
local edit/test and feature-PR permissions cover this work; no deployment or
merge is part of the checkpoint.

## Implementation Decisions

- The pure producer retains every file/move directive and up to 120 source
  lines per file. It scans full changes for counts while retaining only bounded
  hunk text. Validated deleted-line totals come from the existing patch action.
- Existing preview text carries explicit statistics and omitted-line directives;
  no new serialized fields are needed. These are display metadata, not replayable
  patch input. Original input and exact native/browser deltas remain unchanged.
- The TUI first renders all file headers with operations and complete counts,
  then emits up to 80 source lines for each file. Producer omissions and local
  omissions add together. Raw deletions without content show `-?` rather than zero.
- Styled grapheme wrapping preserves paths with spaces, Unicode clusters, and
  code indentation. Decorative gutters shrink on narrow widths. Message and
  progress-summary previews share one margin-removal path, preserving context
  markers and metadata-like code instead of trimming every line.
- The producer implementation and renderer tests move into narrow modules;
  all touched Rust sources stay below 1000 lines.

## Validation

Two original-code regressions fail behaviorally: the producer omits a later
file directive, and the renderer hides later operations after its global budget.
Both pass after implementation.

- `cargo test --locked -p rara-apply-patch -p rara-wasm-core`: 25 pure-patch and
  four browser-preview tests pass; exact deltas retain all content.
- `cargo test --locked -p rara-tools patch::`: 15 native patch tests pass.
- `cargo test --locked -p rara --lib tui::`: 1052 pass, four existing ignored.
- `cargo clippy --locked --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all` and `git diff --check`: passed.

Eleven new tests cover independent budgets, known/unknown deletion counts,
producer/local omission composition, Unicode and widths from 0 through 80,
metadata-like code, and inventory layout. Real native dry-run and apply calls
cross runtime formatting and committed transcript rendering at 80/40/20 columns.
The new compact inventory snapshot was inspected before acceptance; existing
snapshots remain unchanged. An initial style assertion also matched green count
text, so it was corrected to identify diff content by its background as well.

Local default Bazel has the previously recorded external-cache failure resolving
`rules_rust//rust`; configuration and cache were not changed. Remote default
Bazel remains the integration gate.

## Follow-Ups

No implementation follow-up remains within #985. Complete remote CI/review before
integration. A full-diff viewer remains separate queued work; old truncated text
cannot recover file details that were never recorded.
