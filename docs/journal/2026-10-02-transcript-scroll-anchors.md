# Bounded Transcript Scroll Anchors

## Summary

Issue [#920](https://github.com/linkerdog/rara/issues/920) identifies three
connected defects: unbounded scroll debt, bottom-relative offsets that move an
up-scrolled view during appends, and a 16-bit offset that makes long tails
unreachable. This checkpoint builds on the shared visual rows in
[PR #936](https://github.com/linkerdog/rara/pull/936), frozen at `16030caf`.

## Reference Patterns And Plan

- Codex `codex-rs/tui/src/pager_overlay.rs` clamps navigation against measured
  content/viewport height and distinguishes following the bottom when rebuilding
  or adding history. Its remaining local `u16` rendering adapter is not adopted.
- Claude Code `src/ink/components/ScrollBox.tsx` separates manual top/anchor
  navigation from sticky bottom mode and exposes fresh content measurements.
  Its pending-delta drain/coalescing mechanism is outside this scroll-state fix.
- First define explicit tail-following and absolute top visual-row anchoring.
  Keep numeric state separate from wrapping/rendering, refresh measured bounds
  before input, and retain full-width indexes through viewport and selection.
- Prove overscroll, streaming stability, and real long-tail drawing before
  implementation; then cover stale-between-frame bounds and lifecycle resets.

## Implementation And Trade-offs

- A private numeric `TranscriptScroll` owns `FollowTail` or `Anchored(top)` plus
  its last measured geometry. Every delta uses saturating arithmetic and clamps
  to the measured maximum. No sentinel offset or mirrored distance-from-bottom
  field is retained. State modules do not construct terminal widgets.
- The renderer supplies actual shared visual-row counts. Keyboard/wheel dispatch
  and the existing drag-autoscroll tick use one refresh-then-scroll boundary,
  including deltas arriving after content changes but before the next frame.
- Appending rows leaves a manual top anchor unchanged. Tail mode follows the
  newly measured end; manual navigation to the end restores tail mode. Layout
  shrink/reflow clamps an anchor without implicitly making it follow new rows.
- Transcript reset and thread restoration explicitly reset tail-following and
  measurement state. Starting ordinary composer input retains its previous
  explicit follow-tail behavior. Input before the first frame is ignored.
- Global viewport and selection snapshot offsets are `usize`; rendering slices
  the already wrapped rows before handing the small visible window to a
  `Paragraph`. The existing one-row breathing room is preserved.
- The required state-field adaptation touches a 1,922-line test file. It is
  mechanically split into three focused children and a 37-line shared fixture
  file. All 66 test names remain present. Two scroll fixtures adapt to the typed
  model; Unicode fixtures use escapes with unchanged runtime strings. Every
  touched Rust file stays below 1,000 lines; `mod.rs` remains a facade.
- No runtime public protocol, database schema, persistence format, dependency,
  or Bazel configuration changes are made. No snapshots are regenerated.

## Validation

All three initial regressions are behavioral REDs on `16030caf`:

1. Forty PageUp events reach the top, but the next PageDown still displays
   `ROW-00000` instead of moving immediately.
2. Appending twenty stream rows shifts the first visible row from `ROW-00048`
   to `ROW-00068` while manually scrolled up.
3. A 70,000-row production transcript does not render its actual `ROW-69999`
   tail. The corrected regression also checks highlight/copy, a middle window
   beyond 65,535, and navigation back to the actual top and tail.

A mutation omitting input-time layout refresh fails the new between-frame test:
its visible row remains `ROW-00048` while the correctly refreshed reference
shows `ROW-00068`. The mutation is reverted. A temporary missing import during
the type adaptation is corrected; it is not counted as regression RED evidence.

```bash
cargo test --locked --lib transcript_scroll -- --nocapture
cargo test --locked --lib --quiet
cargo check --locked
cargo clippy --locked --all-targets --no-deps -- -D warnings
cargo fmt --all -- --check
git diff --check
bazel test //:rara_unit_tests --test_output=errors
```

The focused filter has sixteen passing cases: seven production render/input
cases, five pure state cases, and four adapted existing guards. The root library
suite reports 1,559 passing tests and one ignored fixture. Default-config Bazel
invocation `98dccf34-0b1f-45d1-b346-2921ef40984f` passes the full root target;
its log confirms the new cases and the same 1,559/one-ignored result. Compilation,
strict all-target Clippy, formatting, and diff checks pass without source
warnings. Cargo's existing macOS large-`__eh_frame` linker warning is unchanged.

## Remaining Work

- [#921](https://github.com/linkerdog/rara/issues/921) still owns incremental
  wrapping/selection caches and redraw coalescing. This implementation refreshes
  full visual rows for correctness; it does not claim bounded per-frame work.
- Anchoring is by top visual-row index, not semantic text identity across width
  reflow, history prepending, or in-place content replacement.
- The scripted renderer/input checks do not prove OS terminal wheel encoding,
  clipboard acceptance, viewport placement, or suspend/resume lifecycle.
- Exact-head remote CI, review, and merge remain separate publication gates.
