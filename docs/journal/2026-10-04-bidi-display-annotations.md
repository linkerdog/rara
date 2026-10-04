# Bidirectional Control Display Annotations

## Summary

Issue [#923](https://github.com/linkerdog/rara/issues/923) had a Unicode formatting
policy gap: invisible direction controls disappeared from the transcript
without exposing the original source distinction. Explicit bidi controls now
appear as visible code-point labels while legitimate joining and emoji
sequences retain their text.

## References And Plan

- Codex `f959e7fc9832dfa0ebfb6542ab1bbf829638ac24`,
  `codex-rs/tui/src/terminal_title.rs`, uses a curated removal policy at the
  presentation boundary. Its title-only exclusions also include joiners and
  variation selectors, so they are too broad for transcript text.
- Claude Code `4b9d30f7953273e567a18eb819f4eddd45fcc877`,
  `src/utils/sanitization.ts`, uses category filtering and NFKC normalization.
  Applying that policy here would change ordinary source and emoji semantics.
- [Unicode 17.0 PropList](https://www.unicode.org/Public/17.0.0/ucd/PropList.txt)
  separates twelve `Bidi_Control` characters from the two `Join_Control`
  characters. Use only the former for annotations.

The implementation reuses the existing stream display boundary, preserves
paste/source text, and maps editor rendering back to original source offsets.
Rendering and copy are verified as well as ingestion; a visible annotation must
not silently become a submitted replacement string.

## Key Decisions

- Labels use mathematical brackets, for example `⟦U+202E⟧`, to avoid Markdown
  link/reference and HTML syntax. They are ordinary display text and survive
  repeated sanitation.
- Raw runtime/tool payloads, draft history, and submitted prompts retain their
  source. Paste still removes terminal escape/control sequences and normalizes
  line boundaries, but does not replace Unicode bidi controls.
- Cursor offsets remain source character offsets. Expanded labels may wrap;
  editing still removes the original character as one unit.
- Other Unicode formatting follows the existing visible-grapheme projection.
  This is not a confusable-text detector, Unicode normalization, or a general
  invisible-character inspector.

## Validation

- Baseline: six new display/editor regressions failed for missing annotations;
  the joining/emoji preservation case already passed. The initial fixture API
  compile error was corrected before recording this behavioral baseline.
- `cargo test --locked --lib tui::`: 994 passed, four existing subprocess fixtures
  ignored by the direct runner. All nine new bidi cases pass, covering the
  16 KiB progress cap after expansion, Markdown finalization, overlay cursor
  positions, masking, source-preserving submission, and copy/highlight agreement.
- `cargo clippy --locked --workspace --all-targets -- -D warnings`: passed.
- `bazel test //:rara_unit_tests --test_arg=tui::`: passed with the default
  repository configuration.
- `cargo fmt --all` and `git diff --check`: passed.

## Follow-Ups

Exact-head CI, review/merge, and physical-terminal acceptance remain separate
delivery gates. Terminal shaping of natural RTL text is outside this change.
