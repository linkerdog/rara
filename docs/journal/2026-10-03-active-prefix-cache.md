# Active Prefix Cache

## Summary

This #921 checkpoint caches active-prefix assembly as well as its wrapped rows.
Long prompts, completed thinking, and non-streaming messages no longer rebuild
on every paint, scroll, composer edit, or response delta. A fixed-size key tracks
their owned inputs and the scalar presentation state. Changed prefixes still
use complete styled-line comparison before replacing wrapped rows.

## References And Plan

Inspected local Codex `f959e7fc9832dfa0ebfb6542ab1bbf829638ac24`
(`chatwidget.rs`, active-cell revision/animation keys) and Claude Code
`4b9d30f7953273e567a18eb819f4eddd45fcc877` (`VirtualMessageList.tsx`,
immutable-message identity caches; `Markdown.tsx`, input-based memoization).

The Long Plan first reproduced production assembly work on long prefixes and
proved a mutation container with nested-edit/replacement tests. It then applied
private input tracking, followed by cold-renderer comparisons, stream and clock
checks, and normal stacked delivery on #954. No public API, protocol, schema,
dependency, or Bazel configuration changes are needed.

## Key Decisions

- `PresentationInput<T>` retains owned data and an identity token. Mutable
  dereference renews the token before exposing nested data. Immutable reads
  retain it; cloned values share identity until either copy mutates. Cached
  tokens retain their allocation, preventing pointer reuse from matching an old
  key. The container is deliberately unsuitable for interior-mutable inputs.
- Only TUI-owned active inputs are wrapped: turn, live sections, snapshot,
  phase detail, planning suggestion, and two follow-up queues. Runtime snapshots
  and persisted turns keep their existing plain data types. Compiler-directed
  conversions at whole-value assignments and boundary unwraps account for most
  of the call-site changes; runtime behavior is unchanged.
- The key also tracks width, theme, phase, busy state, execution mode, thinking
  visibility, interaction selection, response presence, thinking stream identity,
  displayed thinking duration, and whether a history divider is required.
  Composer-only edits and response source revisions are excluded. Repeated
  identical phase details preserve their identity at the setter boundary.
- Thinking duration is captured once for keying and assembly. The cache refreshes
  when its one-decimal display changes, without requiring sleeps in tests.
- Whole runtime snapshot replacement conservatively invalidates the prefix.
  Changed inputs can produce identical styled rows and retain their old wrapped
  block. The fast path does not compare accumulated data or styled rows.

## Validation

Production assembly regressions compiled and failed on the instrumented parent:

| Scenario | Before | After |
| --- | --- | --- |
| Initial paint plus 30 response appends/paints | 31 assemblies | 1 |
| Initial paint plus 20 composer edits/scrolls/paints | 41 assemblies | 1 |

Fixtures include a 1,000-line prompt, 1,000 completed thinking paragraphs, and a
1,000-paragraph non-streaming answer where applicable. Counters measure the
actual `ActiveTurnCell` assembly boundary, not elapsed time or inferred row work.

Cold-renderer comparisons cover nested same-length text/style/role changes,
runtime snapshot and turn replacement, progress, plans, pending questions,
queued input, runtime detail/phase, execution mode, collapse, selection, busy
state, widths including zero/one, finalization, commit/reset, and thinking
append/replacement/replay. An isolated theme test covers semantic and syntax
changes and identical reinstalls. The explicit clock test proves reuse within
one displayed tenth of a second and refresh at the next display value.

The previous thinking-copy test now asserts its four-rows-per-paint upper bound:
unchanged cached frames can copy zero rows. Its growing-stream exact count is
unchanged. No snapshots have been updated.

A controlled mutation removed revision renewal from mutable dereference. The
existing production selection test then copied `old` after a same-length edit
to `new`. The original source was restored byte-for-byte before final checks.

Cargo and the default Bazel TUI target each pass 967 tests (three subprocess
fixtures remain ignored and are exercised by parent tests). Strict workspace/
all-target Clippy, formatting, and diff checks pass. Bazel initially failed on
incomplete external dependency caches and an upstream Git fetch timeout.
Refetching the incomplete repositories restored the default target; the final
run completed in 90 seconds. No Bazel files or configuration were changed. The
existing gold-linker deprecation warning remains unrelated to these changes.

Validation commands:

```bash
cargo test --lib tui::
cargo clippy --workspace --all-targets -- -D warnings
bazel test //:rara_unit_tests --test_arg=tui::
cargo fmt --all -- --check
git diff --check
```

## Follow-Ups

#921 remains open. Changed-prefix assembly/comparison/rewrapping, including
thinking deltas and duration changes, can still traverse older prefix content.
Long mutable Markdown, source-wide reference replay, sanitizer fallback costs,
remote CI/review/merge, and real-terminal acceptance remain separate work.
