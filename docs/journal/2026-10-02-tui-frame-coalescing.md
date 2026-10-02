# TUI Frame Coalescing

## Summary

The first implementation stage for #921 separates ordered event processing
from coalesced painting. Incremental markdown and shared visual-row/selection
caches remain part of the issue; this checkpoint does not complete it.

## Background And References

Before implementation, the local reference sources were inspected:

The inspected reference files were clean in local Codex checkout
`4b9d30f7953273e567a18eb819f4eddd45fcc877` and Claude Code checkout
`ea2046f36d5ee12d39c8e168fc3e5129301afa2b`.

- Codex `tui/src/tui/frame_requester.rs` coalesces requests at the earliest
  rate-limited deadline; `frame_rate_limiter.rs` records actual emission time
  instead of replaying overdue requests in a catch-up burst.
- Codex `markdown_stream.rs`, `streaming/controller.rs`, and
  `streaming/table_holdback.rs` separate append-only source collection,
  stable/mutable rendering, table holdback, and canonical finalization.
- Claude Code `src/ink/ink.tsx` uses leading/trailing render throttling;
  `src/components/Markdown.tsx` advances a stable block prefix and re-parses
  only the mutable suffix. Immutable-history token caching is separate.

Adaptation: use a scheduler owned directly by the single UI event loop. The
existing controller already provides the producer boundary, so a second actor
and request channel would add lifetime/queue overhead without a new consumer.
Use a 60 FPS upper bound while full frame cost remains unresolved. Preserve
runtime ordering and task completion; only repaint requests are coalesced.

## Implementation Plan And Gates

1. Frame scheduling: input is the existing dirty-state projection; output is
   one independently awaited frame deadline. Compare an actor scheduler with
   an event-loop-owned pure model; choose the latter. Enter with the current
   immediate behavior reproduced at a zero interval; exit with deterministic
   burst, final-update, idle, late-frame, and ordered-render regressions.
   Risk: dropping the last dirty update or delaying completion behind painting.
2. Incremental markdown: input is append-only sanitized source; output is a
   stable prefix plus replaceable markdown tail. Compare block-local parsing
   with full newline-gated re-parsing; only block-local work can satisfy the
   cost target. Enter after source-boundary and structural fixture proof;
   exit with parse/byte counts and final-render equality for fences, tables,
   lists, and references. Do not freeze mutable rows to pass cost tests.
3. Shared visual rows: input is committed/active content and explicit layout
   identity; output is reusable styled rows and selection text. Compare
   source-generation caches with per-frame complete hashes; choose explicit
   invalidation plus a correctness fallback where required. Enter with all
   mutation paths mapped; exit with clone/wrap/hash work counts and production
   render/copy/scroll agreement, including histories beyond 65,535 rows.

Preparation: local source/docs edits, normal branch/commit/push/PR delivery,
Cargo checks, and default-config Bazel validation are in scope. No dependency,
configuration, schema, persistence, or runtime protocol change is needed for
stage 1. No worktree or additional agent is used.

## Validation

The zero-interval extraction reproduces old immediate paint eligibility. Five
regressions fail behaviorally: 1,000 requests paint 1,000 times, the ordered
controller/renderer fixture paints 100 times for 100 deltas, a pending deadline
is too early, continuous traffic exceeds 60 paints/second, and a late paint
allows an immediate catch-up frame. Three other guards pass. An initial
test-only Future pinning compile error was corrected; it is not RED evidence.

With the final interval, all nine scheduler regressions pass: six pure-time
cases, two timer/idle cases, and one production-controller/renderer case. The
last case retains all 100 ordered deltas, applies final text and the terminal
runtime event, then paints once with `TOKEN-100` visible in the actual buffer.
No event after that final update is needed to request the trailing frame.

```bash
cargo test --locked --lib frame_scheduler -- --nocapture
cargo test --locked --lib --quiet
cargo check --locked
cargo clippy --locked --all-targets --no-deps -- -D warnings
cargo fmt --all -- --check
git diff --check
bazel test //:rara_unit_tests --test_output=errors
```

The root library reports 1,568 passed and one ignored fixture. Default-config
Bazel invocation `a3f33c08-9c2f-4c91-8c9f-e7dd458c15f7` passes the root target;
its log names all nine new regressions and reports the same result. Compilation,
strict all-target Clippy, formatting, and diff checks pass, without new source
warnings. The unchanged macOS debug-linker large `__eh_frame` warning remains.

The event-loop wiring is source-reviewed; the scripted case exercises the
production scheduler, controller, and renderer, not `run_tui` startup/teardown
on a real terminal. These checks do not establish OS input/resize latency,
viewport restoration, or clipboard acceptance. No snapshots are regenerated.

## Follow-Ups

- Complete stages 2 and 3 of #921 before claiming bounded per-delta/per-frame
  transcript work. The current markdown collector, active cell, full wrapping,
  and selection hash still process accumulated content.
- Keep exact-head remote CI, review, merge, and manual terminal acceptance
  separate from scheduler/model/renderer checks.
