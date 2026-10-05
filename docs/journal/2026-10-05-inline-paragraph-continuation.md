# Inline Paragraph Continuation

## Summary

Formatted root paragraphs now reuse canonical raw-text tails during ordinary
appends. Their visual rows retain a styled prefix through a span cursor, so
source parsing and layout work do not grow with the complete eligible paragraph.
This is another #921 checkpoint after plain paragraph and growing-line reuse.

## Background And Reference Review

An early inline construct previously forced repeated mutable-paragraph parsing.
Two hundred word appends produced 3,015-3,047 source bytes but parsed
304,515-310,947 bytes in 201 passes. Styled rows were correct; the work guard
failed. A separate layout guard copied 305,319-504,108 body bytes and wrapped
306,525-505,314 input bytes for rich prefixes followed by those appends.

Codex's streaming controller at `f959e7fc9832dfa0ebfb6542ab1bbf829638ac24`
retains append-only source but rerenders committed deltas. Claude Code's Markdown
component at `4b9d30f7953273e567a18eb819f4eddd45fcc877` caches immutable lexer results and
samples initial source for syntax. Neither establishes a mutable rich-paragraph
work bound. The implementation instead obtains continuation evidence from the
canonical parser pass, then measures source and layout boundaries separately.

## Key Decisions

- Seed continuation only from a final root-paragraph raw text event followed by
  ASCII spaces. Its last character must be ordinary text or supported sentence
  punctuation. Do not infer appendability from the last span's style.
- Wait for that raw text event after decoded entities, code, and closed inline
  constructs. The locked parser probe confirms they can create separate spans.
  Paragraphs starting with `[` remain canonical: `[id]: ` can disappear into a
  reference definition when an ordinary destination is appended. Later physical
  lines must start with ordinary letters/text; `Plain\n1. ` can still become a
  list only when its content arrives.
- Retain pending spaces separately and extend only the final raw span. New
  parentheses/quotes can close earlier link destinations/titles and require
  replay, as do the existing syntax, structural, and reference exclusions.
  Replay advances the epoch before retained rows can change.
- Pass an explicit append-only row boundary to layout. A retained byte offset
  and span index skip the immutable prefix while borrowing the styled suffix.
  The shared sanitizer/wrapper owns grapheme projection. Changed sanitized bytes
  restore the saved logical-line prefix and use canonical wrapping.
- Count span visits as well as bytes, including empty spans. Keep the three-row
  mutable layout window and existing width/view/epoch invalidation contract.

## Validation

The source guard now parses 22-47 bytes in one or two passes for the original
word cases. Additional emoji, combining-character, and sentence-ending chunks
remain bounded (at most 48 parsed bytes). Rich-line body-copy bytes are zero;
wrapping input is 7,027-8,015 bytes at width 8 or 40,895-42,936 at width 80. A
prefix with 200 bold segments retains its styles without rescanning old spans.

Focused checks include canonical styled rows at every character and two-chunk
split, every ASCII append, definition candidates, unfinished links/titles,
entities, whitespace, local links, and Unicode. Layout checks include cross-span
emoji/variation-selector clusters, normalization replay, late headings,
soft-break completion, compact view, and retained snapshots with 5,000 empty
spans. Existing snapshots are unchanged.

The first full TUI run found that the initial alphanumeric-only seed missed
chunks ending in emoji at the real app entry. The seed was corrected to admit
non-ASCII text and supported sentence punctuation. The source cost matrix now
covers those endings, and the app regression preserves its original byte budget
for plain and formatted prefixes. A second full run caught the empty ordered-list
interruption boundary after adding sentence punctuation. It is now guarded and
covered by focused tests.
All 48 source tests, 17 row tests, and the production app byte-bound test pass
after both fixes. The final full TUI run passes all 1,144 tests; six ignored
subprocess entries are exercised by their parent tests. Strict workspace and
all-target Clippy, formatting, and diff checks pass. Remote CI and terminal
acceptance remain separate from these source checks.

Commands:

- `cargo test --locked --lib tui::markdown_stream -- --nocapture`
- `cargo test --locked --lib tui::render::stream_rows_tests -- --nocapture`
- `cargo test --locked --lib formatted_ -- --nocapture`
- `cargo test --locked --lib growing_single_line_uses_bounded_layout_bytes_through_app_rendering -- --nocapture`
- `cargo test --locked --lib tui:: -- --test-threads=4`
- `cargo clippy --locked --workspace --all-targets -- -D warnings`
- `cargo fmt --all --check`
- `git diff --check`

## Follow-Ups

Issue #921 remains open. Repeated delimiter changes, ineligible paragraphs and
lists, definition/reference-budget replay, control-cleanup replay, large grapheme
tails, and display-normalization fallback still need their own work bounds.
Changed structural prefixes and actual terminal acceptance remain independent
costs/gates. This checkpoint does not satisfy #995's complete dependency.
