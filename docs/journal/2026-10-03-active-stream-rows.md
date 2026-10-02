# Active Streaming Rows

## Summary And Scope

This continues #921 from frozen shared-history head
`bba6139e73815f063cbcde4d1d11c9c90492c9af`. The remaining long response was still
cloned during cell assembly and traversed/rewrapped as one changed active block.
The response now retains stable visual blocks independently of the active
prefix. No runtime protocol, schema, dependency, configuration, cancellation,
clipboard, or terminal-lifecycle contract changes are included.

## References And Plan

Inspected before implementation:

- Codex `ea2046f36d5ee12d39c8e168fc3e5129301afa2b`: source-only newline
  collection and canonical replay in `markdown_stream.rs`; immutable cached
  history and width/revision/continuation/animation live-tail keys in
  `pager_overlay.rs`.
- Claude Code `4b9d30f7953273e567a18eb819f4eddd45fcc877`: `Message.tsx`
  separates static message identity from width, thinking visibility, and latest
  bash-output invalidation. No offscreen freeze is introduced.

The earlier source/shared-row journals transposed these two reference SHAs.
This checkpoint corrects the mapping after checking each local repository's
origin and the target files in its exact commit (`git cat-file -e`). The
implementation patterns and validation results are unchanged.

Long Plan:

1. Extend per-instance instrumentation to the real streaming-cell clone
   boundary on the frozen parent. Exit with long unchanged/growing stream
   behavioral work-count and retained-allocation REDs.
2. Separate the eligible final response from ordinary active prefix assembly.
   Promote source-stable logical rows into shared visual blocks; replace only
   preview/summary rows. Exit with those REDs green and unchanged stream chrome.
3. Verify canonical styled rows, replay/view/width keys, source replacement,
   finalization, selection, and persistent indexes. Update spec/TODO/journal,
   then deliver normal stacked commit/PR with exact-head CI tracked separately.

All steps use local source/docs, focused Cargo checks, default Bazel configuration,
and normal branch/push/PR delivery. No additional schema/environment decision or
history rewriting is needed.

## Key Decisions

- The source collector owns the response layout cache and a replay epoch.
  Append-only byte length is a revision only within that epoch. Replacement,
  finalization, reference invalidation, and conservative fence replay advance
  the epoch, including replay that restyles already displayed code.
- Stable completed code rows can be promoted while an eligible fence remains
  open. On closers, normalization, or highlight fallback, canonical replay
  invalidates those rows instead of retaining an incorrect styled prefix.
- Only the eligible final response is separated. General prefix cards retain
  their existing exact styled-line comparison, ordering, suppression, and
  animation. Retrofitting a revision across every presentation mutation would
  broaden this slice and risk stale output from a missed write.
- Full/compact/width keys are explicit. Compact uses the same four logical head
  rows and styled remaining-row summary; no independent truncation contract is
  introduced. Cold reflow/view changes may rebuild the selected response.
- A persistent binary-carry forest replaces complete block-index copy-on-write.
  Stable subtrees remain shared and cache row counts; at most logarithmically
  many root handles are copied per retained-snapshot append. Joined snapshots
  have bounded production depth rather than one new link per source delta.
- The replaced Markdown wrapper became dead. The unused-code skill required
  checking its history, callers, journal, and TODO intent first. It was a
  superseded implementation, not a reserved feature, so it and obsolete imports
  were removed without suppressing warnings. Stream chrome/style remains exact.
- The production path also supersedes `ActiveCell`; its old display entry point
  and inline stream-cell constructors are retained only as test oracles. The
  obsolete trait is removed. Its declaration exposed existing business logic in
  `cells/mod.rs`, so shared contracts/helpers, ordered segmentation, and helper
  tests are extracted without changing behavior. That file is now a facade;
  this is one #928 boundary, not completion of the larger modularization issue.

## Validation

Four tests compiled and failed behaviorally on the frozen parent with the new
streaming-cell clone meter. Counts below include real stream-body cloning,
visual-row wrapping/text construction, and visible-window cloning:

| Scenario | Parent clone / wrap / text rows | Incremental clone / wrap / text rows |
| --- | --- | --- |
| 20 unchanged frames after a 1,000-paragraph paint | 40,260 / 0 / 0 | 280 / 0 / 0 |
| 200 paragraph appends and paints | 42,770 / 40,600 / 40,600 | 3,566 / 799 / 799 |
| 200 open-fence code-line appends and paints | 23,055 / 20,900 / 20,900 | 2,956 / 204 / 204 |
| Stable styled allocation across 20 appends | Replaced | Retained |

Meters are test-only and per instance. They exclude general prefix assembly and
comparison, Markdown parsing, sanitizer/control-token processing, forest metadata,
and OS terminal output. Zero row hashing is not a guarantee about all source work.

Additional guards compare complete styled lines, text, and display widths with
an independent pre-cache stream-chrome oracle. Character-at-a-time structural
chunks cover paragraphs, lists, references, tables, quoted/indented/top-level
fences, partial closers, Unicode, and CR/NUL normalization. Widths include 0/1/2,
8, 80, 120, and 160. Checks cover full/compact transitions, same-length source
replacement, unchanged reads, finalization without new source, and highlight-limit
replay. Production cells cover prefix/compact changes, thinking visibility,
suppression, finalization, committed boundaries, reset, scrolling, and selection.
The index guard retains 4,096 snapshots with variable-sized blocks and asserts
balanced power-of-two roots, logarithmic root count, exact row order, and old row
identity. Joined-boundary copy and empty joins are covered separately.

Three preliminary failures were fixture mistakes, not product REDs: the tool
role is `Tool`, selection endpoints do not automatically grow with source, and
the first fence row is a language label rather than a highlighted code line.
The fixtures were corrected without changing those production contracts.

Commands:

```bash
cargo test --locked --lib active_stream_tests -- --nocapture
cargo test --locked --lib stream_rows_tests -- --nocapture
cargo test --locked --lib tui::transcript_rows::tests -- --nocapture
cargo test --locked --lib tui:: -- --nocapture
cargo test --locked --lib
cargo check --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
bazel test //:rara_unit_tests --test_output=errors
```

The 20 new focused guards pass within 761 TUI checks. After dead-code cleanup
and facade extraction, root Cargo and default-config Bazel both report 1,626
passing tests and one ignored paid-call fixture. Compilation, strict all-target
Clippy, formatting, and diff checks pass without new Rust warnings. Bazel
invocation: `43c2d657-e4aa-4285-bb13-c7c1bc2871b7`. The existing macOS debug-linker
compact-unwind diagnostic is unchanged. Touched Rust sources remain below 1,000
lines and `cells/mod.rs` contains only declarations/imports/re-exports.

No snapshots have been regenerated. Existing Unicode/70,000-row production
render-highlight-copy guards remain unchanged. Exact-head remote CI, review,
merge, and terminal acceptance are separate from these source checks.

## Follow-Ups

- #921 remains open: general active-prefix/non-streaming/thinking work and long
  mutable/source-wide Markdown fallback still need explicit work bounds.
- Forest metadata and indexed access are logarithmic, not constant-cost.
- Stateful sanitizer/control-token and bounded progress retention remain #923.
- Exact-head CI, review, merge, and real-terminal acceptance are separate gates.
