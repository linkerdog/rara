# Incremental Streaming Markdown

## Scope And References

This is the next source/rendering stage of #921, following the coalesced-frame
checkpoint. It does not complete the full issue's historical-row reuse gate.

Before implementation, the inspected local references were Codex
`ea2046f36d5ee12d39c8e168fc3e5129301afa2b` and Claude Code
`4b9d30f7953273e567a18eb819f4eddd45fcc877`:

- Codex's source collector commits newline boundaries without rendering.
  `streaming/render.rs` retains completed top-level blocks, while its single
  parser pass tracks offsets in `markdown_render/streaming.rs`. Tables are
  held back and reference definitions explicitly invalidate earlier blocks.
- Codex's open-fence path retains syntax parser state for complete code lines;
  closing fences and highlighting limits return to the canonical renderer.
- Claude Code's `StreamingMarkdown` retains a stable block prefix and lexes
  only the mutable suffix. Source replacement resets that boundary. Its
  immutable-message token cache is separate from streaming state.

Adaptation: keep one source/render cache per agent/thinking stream. Append
only new source during event reduction, materialize on presentation access,
retain completed block rows by moving rather than cloning them, and replace
the structural tail. The complete-message renderer remains the oracle.

## Plan And Preparation

1. Reproduce work growth and structural divergence at the frozen parent
   `a3d8d764e3a8fa32d198c063a100fc0521a69238`. Use per-instance parse/byte
   counters and styled-line equality, not timing thresholds. Exit with a
   behavioral RED and a verified parser-offset fixture.
2. Implement append-only source and stable/mutable rendering, with explicit
   table holdback, reference/replacement invalidation, and canonical
   finalization. Validate partial chunks, fences, list tightness, setext
   headings, tables, links, Unicode, and unchanged presentation reads.
3. Integrate borrowed stream rows into the live presentation path, validate
   agent/thinking transitions and final controller output, and record exact
   source/CI boundaries. Do not claim whole-transcript row reuse from this cache.

Full-source newline-gated rendering is rejected because it retains quadratic
work. A block-local cache is preferred; long mutable blocks and source-wide
dependencies must be separately counted, not hidden by freezing wrong output.

Local source/docs edits, focused Cargo checks, default-config Bazel, and normal
branch/commit/push/PR delivery are in scope. No dependency, schema, protocol,
configuration, or worktree change is needed. The oversized renderer's tests
and table helpers are mechanically extracted without moving the snapshot
directory or changing the existing 17 snapshot tests.

## Implementation And Decisions

- `MarkdownStreamCollector` owns append-only visible source and one row cache.
  Ingestion examines only the new delta's last newline and performs no parsing.
  Presentation access refreshes changed source once; unchanged reads borrow the
  same rows. One refresh may parse both the complete tail and its stable-boundary
  prefix, plus an incomplete preview; this is not a one-parser-call promise.
- A parser-offset iterator tracks top-level block boundaries in the canonical
  writer's event pass. Only newline-completed source can advance the stable
  boundary; incomplete structural text is replaceable. Retained offsets include
  physical-line indentation skipped by parser ranges. Root newline/output state
  is resumed across completed blocks instead of guessing separator rows.
- A confirmed table holds its enclosing root block and all following source
  until full canonical finalization. Reference definitions reset the stable
  boundary and explicitly select full-source replay. Replacement resets all
  source, row, fence, table, and formatting boundaries.
- An unindented top-level open fence retains syntax parse/highlight state for
  completed code lines. Partial-line preview uses cloned syntax state, not
  cloned old code or rows. Closer candidates, normalization, theme changes, and
  highlighting limits fall back to canonical replay; exceeding the existing
  512 KiB/10,000-line limits removes earlier colors once, then uses plain rows.
  Identical theme installation does not invalidate syntax state.
- Agent and thinking presentation borrow the collector's rows. The former
  `committed_lines`/`display_lines` copies are removed. The old blank-line helper
  has no remaining caller or planned activation after this replacement, so it
  is removed rather than suppressed.
- Canonical finalization remains one full-message pass. This source cache does
  not remove downstream active-cell or whole-history row construction.

## Validation

The original collector was instrumented at its actual parser calls on frozen
parent `a3d8d764e3a8fa32d198c063a100fc0521a69238`. Three focused assertions were
behavioral REDs: completed paragraphs parsed quadratic source bytes, unchanged
preview reads parsed again, and final table rows differed from canonical styled
rows. Loose-list coverage already passed and is not claimed as a baseline RED.

| Workload | Before | Corrected source-cache work |
| --- | --- | --- |
| 200 completed paragraphs, 4,800 source bytes | 400 parses / 964,800 parsed bytes | 399 parses / 14,352 parsed bytes / 1,193 rendered rows |
| 100 unchanged presentation reads | Parse count increased from 2 to 102 | No additional parse, row construction, or stable-text allocation change |
| 1,000 deltas before presentation | Eager source parsing | Zero Markdown parses; one parse on first read |
| 200 open Rust fence lines, 3,017 source bytes | Intermediate block-only cache: 202 parses / 303,126 parsed bytes / 20,303 rendered rows | 3 parses / 49 parsed bytes / 205 rendered rows / 3,016 fence bytes examined |
| 500 table body rows after confirmation | Stale frozen header/delimiter final output | No additional parse before one canonical finalization |

The open-fence RED describes the intermediate block-cache implementation, not
the original parent's counter values. Unknown-language and unlabelled fences
also stay linear in this fixture (73/41 parsed bytes and 205/204 rendered rows).
These counters measure Markdown parser input, generated rows, and fence scans;
they do not measure arbitrary syntax-engine complexity or sanitizer work.

Adjacent-block tests found two bugs during implementation: parser offsets
dropped indented-code semantics after a retained prefix, and independently
joined blocks lost the leading separator after an empty code fence. Preserving
physical-line offsets and canonical writer root state fixes both. The styled
oracle remains unchanged; 100 adjacent-block combinations and six structural
documents at chunk sizes 1/3/7/19/1000 compare complete styled rows throughout.

Additional coverage includes partial headings/Setext, loose lists, quoted
tables, reference invalidation, source replacement, multiline syntax previews,
both highlighting caps, and conservative fence/normalization fallbacks.
Production controller/buffer tests retain all ordered deltas, hold table and
following text until final output, reuse unchanged agent/thinking rows, and
commit thinking exactly once before response text. The fixture uses the actual
`Completed` projection after the terminal session event; `TurnFinished` alone
is not treated as a new completion policy. Earlier fixture import/completion
mistakes are not counted as product RED evidence.

Checks on the final source tree:

```bash
cargo test --locked --lib markdown_stream -- --nocapture
cargo test --locked --lib --quiet
cargo check --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --all
git diff --check
bazel test //:rara_unit_tests --test_output=errors
```

The focused filter has 30 passing tests. The full library has 1,594 passing
tests and one ignored isolated child fixture. Default-config Bazel passes with
invocation `ff831cb9-0d7b-4109-90bb-138218671b97`. Compilation, strict Clippy,
formatting, and diff checks are clean. The existing macOS debug-linker
`__eh_frame` compact-unwind warning is unchanged. No snapshot was regenerated;
all touched Rust sources remain below 1,000 lines.

## Follow-Ups

- Long single paragraphs/lists, quoted/indented fences, and source-wide
  reference fallback still require explicit mutable-tail/full-source work;
  this checkpoint does not promise O(new-delta) work for arbitrary Markdown.
- Retain #921's complete-history clone/wrap/hash reuse gate and production
  render/copy/scroll work-count acceptance.
- Stateful sanitization remains owned by #923; markdown counters do not claim
  a bound on the existing control-token scrubber or terminal sanitizer.
- Remote CI, review, merge, and real-terminal acceptance are separate gates.
