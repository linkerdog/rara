# Streaming List Items

## Problem And Reference Review

Two hundred growing list items contain 4,027-7,000 source bytes but cause
407,427-703,500 parsed bytes and approximately 20,100 generated rows. Canonical
styled output agrees; the deterministic work regression fails on repeated
parsing. This continues #921 after #1050.

Local Codex at `f959e7fc9832dfa0ebfb6542ab1bbf829638ac24` accumulates source at
newline boundaries and carries list numbering/indentation in its writer.
Claude Code at `4b9d30f7953273e567a18eb819f4eddd45fcc877` caches complete lexer
results. Neither provides retained mutable-list item boundaries. The locked
pulldown-cmark probe exposes root item offsets and confirms that later blank
lines can introduce paragraph events throughout a previously tight list.

## Key Decisions

- Capture root-item offsets, numbering, and the last item's row boundary during
  the canonical writer pass. Preserve completed rows before its pending row.
- Resume parsing from the final item with a synthetic paragraph item before it.
  That item exposes list-wide tightness even when the real suffix has only code
  or quotes. A blank separator preserves an already-loose list; remove all
  synthetic events before writing. A block-only initial list needs one extra
  parser probe to establish its spacing context.
- The writer can append an HTML-first item to the previous pending row. Save
  that row as the fragment seed, including indentation and style. Snapshot only
  the final item, avoiding repeated copies of a growing row inside one pass.
- Validate the actual marker separator and reject synthetic-prefix spill:
  `*` becoming `**` must return to canonical parsing, not become a lazy list
  continuation. Root changes, newly loose spacing, tables, new definitions, and
  reference-budget uncertainty invalidate the epoch before replay.
- Count synthetic parser bytes and pending-row seed copies explicitly. Bounds
  depend on the mutable item and boundary row; a long single item is a separate
  remaining cost.

## Validation

The four original 200-item cases now parse 11,616-17,945 bytes and generate
597-602 rows. Pending-row seed copies total 6,930-15,483 bytes. Additional quote,
multi-paragraph, and code-only loose lists satisfy bounds relative to their
source bytes and canonical row count. The row budget is three times the final
canonical rows, accounting for two item renders and the pending boundary row.

The real app path ingests 6,000 bytes across 200 paints: 15,751 parser bytes,
10,503 pending-row seed bytes, 600 wrapped logical lines, and 18,189 wrapping
input bytes. These instrumented source/layout counters exclude terminal IO and
unrelated app work; they are not wall-clock latency claims.

Focused checks pass: 53 source tests, 18 row tests, and the production app guard.
The final full TUI run passes 1,151 tests with six subprocess entries exercised
through parent tests. Character and two-chunk comparisons cover numbering,
indentation, Unicode, references, late syntax, source replacement, and table
holdback. An 81-case block-combination matrix covers quotes, code, HTML,
comments, definitions, and empty items. Retained visual-row allocation agrees
with independent canonical styling/wrapping. Strict workspace/all-target Clippy,
formatting, and diff checks pass. No snapshots were regenerated.

The block matrix exposed both hidden tight-to-loose transitions and HTML writes
to a prior pending row. Broad tests then caught incomplete markers being
absorbed by the synthetic item; the explicit separator/spill guard fixes that
boundary. All corresponding regressions remain in the suite.

Commands:

- `cargo test --locked --lib tui::markdown_stream -- --nocapture`
- `cargo test --locked --lib tui::render::stream_rows_tests -- --nocapture`
- `cargo test --locked --lib growing_list_ -- --nocapture`
- `cargo test --locked --lib tui:: -- --test-threads=4`
- `cargo clippy --locked --workspace --all-targets -- -D warnings`
- `cargo fmt --all --check`
- `git diff --check`

## Follow-Ups

Repeated inline delimiters, growth within one unresolved item, reference/control
replay, changed structural prefixes, and real-terminal acceptance remain
independent #921 requirements. The mutable pending row can also be large in
bytes. This checkpoint does not close #921 or satisfy #995's full dependency.
