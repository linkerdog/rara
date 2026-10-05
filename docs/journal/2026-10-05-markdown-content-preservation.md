# Markdown Content Preservation

## Summary

Issue [#983](https://github.com/linkerdog/rara/issues/983) covers task markers,
table wrapping with inline styles, and image destination text. The owning
contract is [streaming transcript](../features/streaming-transcript.md).

## Background And Reference Adaptation

Inspected local Codex `f959e7fc9832dfa0ebfb6542ab1bbf829638ac24`, particularly
`codex-rs/tui/src/markdown_render.rs`: styled cell collection, shared wrapping,
content-aware allocation, and vertical key/value fallback. Inspected local
Claude Code `4b9d30f7953273e567a18eb819f4eddd45fcc877`, particularly
`src/components/MarkdownTable.tsx` and `src/utils/markdown.ts`: styled wrapping,
width allocation, vertical layout, and image destination display.

Adapt these boundaries to the existing canonical writer and transcript wrapper.
Keep the existing pipe grid and alignment when it fits. Use vertical fields only
when separators and minimum grapheme widths cannot fit; do not import the
references' larger readability heuristics, terminal hyperlinks, or table borders.

## Plan And Boundaries

1. Prove current failures with focused canonical-renderer regressions, then
   enable task events and display image destinations. Include tight, loose,
   nested, empty, and linked-image cases before leaving this stage.
2. Preserve styled cell lines, measure sanitized display content, allocate the
   grid budget after indentation, and use shared wrapping plus vertical fallback.
   Exit with complete cell data and bounded rows across narrow/wide viewports.
3. Review snapshots and buffer styles, compare streaming finalization at chunk
   boundaries and widths, and run formatting and strict workspace lint checks.
   Record results and publish the independent main-based fix.

Only local source/tests/docs and the authorized feature branch/PR are mutated.
No dependency, public API, database, terminal lifecycle, or Bazel configuration
changes are required. Regression tests are the code-level feasibility proof.

## Implementation Decisions

- Enable parser task events and replace the pending marker, including loose
  paragraphs that have already created their line. Keep nested indentation and
  materialize empty checked/unchecked items.
- Track image destinations separately from the enclosing link. Preserve alt
  styles and show both image and enclosing link targets. Local links within alt
  text still use canonical destination labels; nested images retain each URL.
- Store sanitized styled cell lines and route all inline spans, including link
  suffixes, into the current cell. Inline code patches the current inline style.
- Compute natural widths and indivisible-grapheme floors. A binary-searched
  common cap leaves short columns at natural width and shares remaining space
  among larger columns; work does not grow with each discarded display column
  of an oversized cell. Account for enclosing indentation before allocation.
- Use shared transcript wrapping for cells and vertical fields. Preserve grid
  alignment on every continuation row. No second truncation algorithm remains.
- Keep existing source storage and streaming cache boundaries. Confirmed tables
  still wait for finalization; task/image previews use the canonical writer.

## Validation

Five focused regressions fail behaviorally on the unchanged writer: literal
task markers, missing image URLs, ellipsis truncation, misplaced table link
targets, and narrow-grid overflow. They pass with the implementation.

The final fourteen focused tests cover tight/loose/nested/empty task items,
empty/linked/nested images, local link labels, styled cell wrapping, CJK,
combining marks, joined emoji, bidi display labels, header-only/empty-header
fallbacks, and nested table indentation. Actual Ratatui buffers preserve text
and style at widths 2/5/8/12/16/24/40/80; width 1 follows the shared oversized
grapheme replacement policy. Streaming task/image previews and final table
output agree with the canonical writer at every character split and widths
8/24/80. Existing table holding, cache, selection, and stream work-count tests
remain green.

Reviewed the changed task-list/comprehensive snapshots, replaced the narrow
truncation snapshot with wrapping, and added aligned/vertical table snapshots.
Two intermediate test fixtures were corrected: a width of 5 still fits a
two-column minimum grid, and nested hyperlinks are not valid parser input for
the expected outer-link rendering. No parser extension was added for either.

```bash
cargo test --locked --lib tui::markdown_render::content_tests -- --nocapture
cargo test --locked --lib tui:: -- --nocapture
cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings
cargo fmt --all -- --check
git diff --check
```

- Complete TUI suite: 1059 passed, 4 existing ignored tests.
- Strict workspace/all-target Clippy: passed without warnings.
- Formatting, whitespace checks, and touched Rust source-size checks: clean.
- Default Bazel remains a remote acceptance gate; the previously recorded
  local external-cache failure is not bypassed with configuration changes.

## Follow-Ups

No implementation follow-up remains for this issue. Remote CI/review is still
required before integration. Buffer tests establish text layout and styles;
they do not prove font glyph support in every physical terminal. Image loading,
terminal hyperlinks, table readability heuristics, and existing streaming work
bounds remain outside this change.
