# Growing Physical Line Layout

## Scope

Continue #921 after [plain paragraph streaming](2026-10-05-long-mutable-markdown.md).
The source collector can avoid reparsing ordinary paragraphs, while the response
layout still clones and wraps a complete growing physical line on each revision.
Row counts alone conceal this remaining work.

## Reference Review

Codex's wrapping utilities map wrapped rows back to source ranges and distinguish
first-line chrome from continuation content. Its streaming controller separates
mutable rows from queued/emitted stable rows and explicitly rebuilds on resize.
Its complete-line wrapping is not itself an incremental-byte bound. Claude Code's
Ink wrapper delegates to Bun/wrap-ansi for full string wrapping; this provides no
evidence of an append-only layout guarantee either.

## Implementation

The source collector explicitly marks plain paragraph rows. The layout cache
retains all but the last three visual rows of a growing eligible physical line,
using ranges returned by the existing shared wrapper. The tail uses borrowed
source spans, including remaining response chrome, instead of cloning the whole
logical line. Completed physical lines promote their remaining rows once.

The three-row window retains the last grapheme, its preceding word fragment,
and the row that fragment can move back into when the grapheme width shrinks.
Width, view, and replay-epoch changes reset it. If sanitization changes raw byte
positions, restore the retained snapshot from before that logical line and use
canonical wrapping until the line ends. This preserves normalization and avoids
using sanitized offsets against different source bytes. No complete-prefix
hash/comparison is needed on the normal append path.

## Validation

A deterministic regression first demonstrated quadratic input-byte work. After
200 deltas, the 3,000-byte word fixture previously cloned 301,300 body bytes and
passed 302,500 bytes to wrapping at either width. It now borrows the body suffix
and passes 6,985 bytes at width 8 or 40,546 bytes at width 80. The 5,000-byte
Unicode fixture decreases from 503,500 wrapping-input bytes to 11,173/73,913.
Counters measure these production boundaries, not every allocation inside the
wrapper or terminal output; the byte budget includes the mutable visual window.

Fourteen focused response-row tests pass, including canonical styled/text/width
checks at every character for words, unbroken words, Unicode graphemes,
normalization, physical-line completion, late syntax, and compact transitions.
Retained allocations and old snapshots remain stable through appends, then
invalidate correctly on width/view/source replacement. Production application
and isolated theme regressions are included in the full TUI validation.

```sh
cargo test --locked --lib tui::render::stream_rows_tests -- --nocapture
cargo test --locked --lib tui:: -- --test-threads=4
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --check
```

The full TUI run passes 1,138 tests with six parent-driven child fixture entries
excluded from direct execution. Final review shares response chrome between the
canonical and suffix paths; all 14 row regressions pass after that extraction.
Strict workspace/all-target Clippy, formatting, and diff checks pass. Remote
default Bazel and CI remain delivery gates.

## Follow-Ups

Arbitrarily large individual grapheme clusters can enlarge the mutable window
in bytes. Normalization fallback, ineligible long paragraphs/lists, reference
replay, complex control cleanup, changed prefix inputs, and terminal acceptance
remain open under #921. The next issue's dependency is not treated as satisfied
by this checkpoint; see [TODO](../todo.md).
