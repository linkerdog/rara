# Streaming Reference Context

## Summary

Continue #921 by ending permanent full-source Markdown replay after a reference
definition. Retain an owned parser-derived reference context and reuse stable
source/visual rows while ordinary blocks arrive. New or still-mutable definitions
invalidate the prefix; canonical finalization and table holdback remain intact.

## Background And Plan

Codex `f959e7fc9832dfa0ebfb6542ab1bbf829638ac24` separates newline source commits
from stable/tail rendering and holds unresolved tables. Claude Code
`4b9d30f7953273e567a18eb819f4eddd45fcc877`,
`src/components/Markdown.tsx::StreamingMarkdown`, parses the growing suffix and
retains the completed prefix. Neither inspected pattern supplies this project's
reference-context and canonical resource-budget contract directly.

The local pulldown-cmark 0.13.4 implementation exposes parsed reference
definitions and a broken-link callback. A standalone probe verified owned
definitions, Unicode case folding, and first-definition precedence before
implementation. The staged plan was to reproduce production parser work;
implement reference-aware stable boundaries and invalidation; then validate
styled output, visual-row reuse, and native integration before publication.

Local source/docs edits, the already-resolved unicase dependency, default Bazel,
and ordinary branch/PR publication use existing task authorization. No protocol,
persistence, or Bazel configuration change is required.

## Key Decisions

- Preserve decoded destinations/titles from the canonical parser. Use the same
  `unicase` comparison for callback labels instead of reconstructing Markdown
  definitions or assuming ASCII-only labels.
- Keep definitions in the mutable suffix replaceable: appending a title or
  invalidating a provisional destination must revise earlier links. Newly
  encountered suffix definitions trigger full-source replay so duplicate
  definitions preserve the first definition's precedence.
- The stable-prefix rendering pass uses the full definition context, including
  forward references. New definition replay increments the source epoch so
  wrapped rows and selection cannot retain stale link styles or destinations.
- Bound potential reference expansion by the closing-bracket count times the
  largest decoded destination/title size. Check both the whole document and each
  suffix/stable-prefix pass against their own `max(source bytes, 100,000)` budget.
  Otherwise preserve full-source parsing until reset; renewing the parser budget
  separately for each block would change rendering as the document grows. The
  100,000-byte floor alone is too conservative for long, safely expandable
  documents, so it is not used as a fixed cutoff.
- Accumulate bracket counts from appended source only. Owned definitions add
  storage proportional to definitions, not a copy of the full message. New
  definitions still incur canonical source-wide work and definition copying.
- A held table also holds definitions after it. Re-rendering its visible prefix
  must discard those hidden definitions, matching the existing holdback policy.

## Validation

Two behavioral work-count tests failed at main `db9de785` before implementation:

| Workload | Before | After |
| --- | --- | --- |
| 200 referenced paragraphs, 4,227 source bytes | 427,554 parsed bytes; 40,000 generated rows | 12,666 parsed bytes; 1,193 generated rows |
| 200 appends after a late definition revises 1,000 paragraphs | 2,448,200 additional parsed bytes | 7,192 additional parsed bytes |

These counters cover parser input and generated rows, not total CPU time or the
definition map's allocations. Reference-scan bytes are counted separately for
both append bookkeeping and fragment guards. A production paint test retains styled row
identity and adds 800 wrapped logical rows across 200 reference-bearing appends.

Character-boundary oracle tests cover Unicode labels, whitespace, duplicate and
late definitions, multiline titles, provisional definitions becoming invalid,
images, lists, quotes, fences, and escaped destinations/titles. Separate checks
exercise expansion exhaustion followed by budget growth, source replacement,
and table holdback/finalization. Cold and appended documents exercise budget
differences between the complete source and independently parsed prefixes or
tails. A new table guard caught hidden definitions
affecting the prefix during implementation; the prefix now resets that context.

A subsequent 3,000-paragraph guard exposed 59,688,759 parsed bytes for 171,027
source bytes in the initial fixed-floor budget policy. Document- and
fragment-specific budgets replace that policy: the final implementation parses
512,967 bytes, scans 683,994 reference-budget bytes, and generates 17,993 rows.

Final validation:

- `cargo test --locked --offline --lib reference_context -- --nocapture`: nine
  focused regressions passed.
- `RARA_HOME=/tmp/rara-reference-test-state cargo test --locked --offline --lib
  tui::`: 994 passed, four existing ignored subprocess fixtures.
- `cargo clippy --locked --offline --workspace --all-targets -- -D warnings`:
  passed.
- Default Bazel `//:rara_unit_tests`: 1,874 passed, five existing ignored tests,
  with no retry. The nominal `--test_filter=tui::` did not filter this Rust
  harness; the reported result is the full target. The existing default gold
  linker warning remains unchanged.
- Formatting and diff checks passed. No snapshots were regenerated. The
  generated module lock only adds direct unicase aliases and updates input
  hashes/build maps; Cargo adds one dependency edge at an existing version.

The initial default Bazel attempt stopped before tests
because an external repository cache was missing; after restoring that cache,
the dependency resolver timed out fetching the locked nucleo Git revision.
The exact revision was restored from the verified local Cargo mirror and the
same default target passed. No source or build configuration was changed to
work around the dependency fetch.

## Follow-Ups

Long mutable paragraphs/lists, definition-change and expansion-limit replays,
complex control-token replay, structural prefix changes, and physical-terminal
acceptance remain separate #921 work. This checkpoint does not claim an
unconditional O(new-delta) bound for arbitrary Markdown. Remote CI, review, and
merge remain delivery gates.
