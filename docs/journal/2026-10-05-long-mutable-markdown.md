# Long Mutable Markdown Work Bounds

## Summary

An eligible plain paragraph now extends the rows from its first canonical parse
instead of reparsing its accumulated source on every presentation. Completed
physical lines are retained by the existing stream row cache. This is a scoped
checkpoint for #921, not closure of its arbitrary-Markdown or long-line layout
work. The canonical contract is [streaming transcript](../features/streaming-transcript.md).

## Background

The deterministic regression appended 200 chunks to a single paragraph:

| Input | Source bytes | Previous parsed bytes | Current parsed bytes | Current eligibility bytes |
| --- | ---: | ---: | ---: | ---: |
| Words without newlines | 3,000 | 301,500 | 15 | 3,000 |
| Words with soft breaks | 4,800 | 482,400 | 24 | 4,800 |

The soft-break case previously generated 20,100 logical rows; it now touches
200. These are source/cache work counts, not end-to-end latency measurements.

Codex's source collector separates appended source from newline-gated output;
production parsing belongs to its stream controller. That separation is useful,
but copying complete-source rendering would retain the measured cost. Claude
Code's Markdown renderer bypasses lexing for plain text and caches immutable
messages. Its first-500-character syntax heuristic cannot establish canonical
equivalence for later syntax, so this path examines all candidate/new source.

## Key Decisions

- Initialize only after canonical rendering of an eligible root paragraph.
  Extend its last owned text span and append new logical lines in place.
- Check every appended character before mutating rows. Structural/inline syntax,
  indentation, blank lines, hard-break spaces, controls, and BOMs return to
  canonical rendering. Candidate rejection is remembered for that mutable block.
- Retain trailing spaces outside visible text until subsequent content makes
  them internal. This preserves parser trimming without rescanning old text.
- Keep source-stable block offsets unchanged while promoting completed plain
  rows. On later syntax, advance the row replay epoch so tentative visual rows
  cannot survive paragraph-to-heading or other structural reinterpretation.
- Replacement and theme replay reset the eligibility state. Reference budget
  and mutable-definition invalidation still run before this fast path.

## Validation

The initial work regression failed against the previous implementation. Focused
source and visual-row checks cover canonical styled rows at every Unicode-safe
two-chunk split and character boundary; all ASCII insertion boundaries; late
syntax; reference contexts; pending spaces; BOM/CRLF/control fallback; replacement;
theme replay; and full/compact rows at widths 1, 8, and 80. Retained snapshot
allocation checks prove completed lines are reused and replay preserves old views.

Commands:

```sh
cargo test --locked --lib plain -- --nocapture
cargo test --locked --lib tui:: -- --nocapture
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo fmt --check
git diff --check
```

The focused filter passes 22 tests. The final full TUI run passes 1,133 tests
with six parent-driven child fixture entries excluded from direct execution.
An earlier full run encountered one external-editor PTY timeout while waiting
for a post-restoration key; its isolated eight-scenario recheck and the full
rerun pass without changing that fixture or its assertions. The cause of that
single intermittent failure is not established by the Markdown tests.
Strict workspace/all-target Clippy, formatting, and diff checks pass. Remote
default Bazel and CI remain delivery gates.

## Follow-Ups

- A single growing physical line still rebuilds its visual wrapping. Logical-row
  counts alone do not measure that byte/copy cost.
- Ineligible long paragraphs/lists, definition changes, reference-expansion
  fallback, and complex control cleanup retain canonical replay costs.
- Changed structural prefix work and real-terminal acceptance remain open.
  #995 keeps its explicit #921 dependency. See [TODO](../todo.md).
