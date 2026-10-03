# Shared Transcript Wrapping

## Summary

Issue [#918](https://github.com/linkerdog/rara/issues/918) is addressed by one
grapheme-boundary layout primitive with explicit transcript word-wrapping and
composer indent/grapheme-wrapping profiles. Transcript rendering, row counts,
viewport slicing, and mouse selection now consume materialized visual rows.
This checkpoint is stacked on composer geometry PR #935 at `99037f28`.

## Background And Reference Adaptation

The previous viewport counted `ceil(line.width / width)` but rendered with
Ratatui word wrapping; selection independently split characters. These are
different rows, not merely different metadata.

Before implementation, the local Codex `tui/src/wrapping.rs` source was inspected
for shared wrapping ranges, styled-row materialization, and legal grapheme
boundaries. Claude Code `src/utils/Cursor.ts` was inspected for `MeasuredText`,
wrapped lines, source offsets, and display-column navigation. The adaptation
shares those boundaries without importing model/runtime dependencies or changing
draft storage to a new offset type. URLs are treated as unsplit tokens when
they fit, and as grapheme-safe long words when they exceed a row; no synthetic
hyphenation is inserted.

## Scope And Key Decisions

- `text_wrap.rs` owns width measurement, tab policy, and source ranges with
  word/grapheme profiles. `composer_text.rs` retains its existing cached layout
  and character-offset API, but only exposes legal grapheme boundaries to
  vertical navigation.
- `transcript_text.rs` preserves line/span styles while materializing visual
  rows. A grapheme crossing a span boundary uses its first contributing span's
  style. Tabs expand before transcript wrapping; an indivisible grapheme wider
  than the whole transcript row uses a one-column replacement glyph.
- The viewport slices visual rows directly, including partial logical lines.
  It no longer estimates counts or asks `Paragraph` to wrap again.
- Selection has no wrapping implementation of its own. Highlight and copy use
  the same whole-grapheme selection boundary. Cache identity hashes all visual
  row content, so same-sized interior edits cannot reuse stale selected text.
  This addresses the selection-cache portion of #923, not its other concerns.
- The already locked `unicode-segmentation` version is now a direct dependency;
  Cargo lock changes only the root dependency list. `bazel mod deps` regenerates
  the derived extension lock. Its 34 changed repository versions were stale
  relative to the existing Cargo lock, and every newly generated repository
  version matches that unchanged package list. No Bazel configuration or Cargo
  package version was changed.

## Validation

Three behavioral REDs on the base head used production viewport rendering:

1. `aaaa bbbb cccc dddd` at width 8 draws four rows but counts three.
2. A tail window beginning at row 3 skips `dddd` and shows only `TAIL`.
3. Highlighting the second displayed row `bbbb` copies `b cc`.

Two additional guards were verified with minimal production mutations, then
restored: removing grapheme snapping leaves the first emoji cell unhighlighted;
hashing only transcript edges copies `old` after a same-sized middle edit to
`new`. Both fail behaviorally, not by compilation or assertion changes.

The corrected tests consume visual rows; old internal binary-search/inner-scroll
assertions are replaced by the actual partial-window text. No snapshots are
regenerated. Coverage includes widths 1/2/3/4/8/12/80/120/160, prose, explicit
empty lines, trailing newlines, tabs, CJK, combining marks, joined emoji, flags,
long words, URLs, styles spanning clusters, alignment, non-zero scroll, and a
full `TuiHarness` follow-tail/selection check with and without a sidebar.

Commands and current corrected results:

```bash
cargo test --locked --lib tui:: --quiet
cargo test --locked --lib --quiet
cargo check --locked
cargo clippy --locked --all-targets --no-deps -- -D warnings
cargo fmt --all -- --check
git diff --check
bazel mod deps
bazel test //:rara_unit_tests --test_filter=tui::render::viewport::tests --test_output=errors
```

- TUI: 682 passing tests; focused viewport and selection checks cover 8 and 5
  tests respectively.
- Root library: 1547 passing tests and one ignored fixture.
- Compilation, strict all-target Clippy, and formatting are clean after fixing
  slice iteration in the adjusted internal tests. The existing macOS debug
  linker `__eh_frame` warning is unchanged.
- The Bazel Rust target does not apply `--test_filter`; its test log reports the
  whole root library suite. The first passing run warned of a concurrent source
  modification during compilation, so it is not used as frozen-source evidence.
  The second run on stable source passes: the test log reports 1547 passing
  tests, one ignored fixture, and the new viewport/selection guards. Exact-head
  remote CI remains a separate gate.

## Follow-Ups

- This is layout/copy correctness, not #921's incremental streaming/render
  complexity fix. Full visual-row materialization remains proportional to the
  transcript size; track caching and bounded redraw work in TODO.
- #920 scroll-state clamping, #924 grapheme-aware editing, and #925 terminal
  lifecycle/manual acceptance remain separate issues.
- Clipboard transport and terminal emoji-width policy still need acceptance in
  the actual terminal environment. Buffer tests do not prove clipboard delivery
  or emulator behavior.
- PR #935 and this stacked fix must be integrated before claiming main contains
  the behavior. No issue is closed by this checkpoint alone.
