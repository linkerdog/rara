# Live Thinking Row Slot

## Summary

This #921 checkpoint retains both static sections around a live-thinking cell.
Thinking deltas and displayed-duration changes update only the thinking block,
without assembling, comparing, or wrapping the rest of the active turn.

## Background And Plan

The preceding active-prefix cache skipped unchanged frames, but a thinking
source revision or clock tick invalidated the whole prefix. The production
regression rebuilt long prompts and prior answers on each thinking delta.

Inspected local Codex `f959e7fc9832dfa0ebfb6542ab1bbf829638ac24`
(`chatwidget.rs`, separate active-cell revision and animation identity) and
Claude Code `4b9d30f7953273e567a18eb819f4eddd45fcc877`
(`VirtualMessageList.tsx`, immutable-message identity; `Markdown.tsx`, retained
stable content beside a changing suffix). The adaptation keeps the existing
thinking position while separating its dynamic rows from neighboring content.

The Long Plan reproduced production assembly/wrapping and explicit-clock
failures first, separated static and dynamic layout identities, then validated
styled rows, retained allocations, cross-section selection, and lifecycle
transitions. Delivery remains stacked on #955.

## Key Decisions

- The structural prefix key tracks thinking stream presence and whether its
  rendered source is empty. These transitions can add or remove a cell and its
  separators. Source identity and displayed duration belong to the separate
  thinking key, alongside width and theme.
- Active assembly records a thinking slot between two static line vectors.
  Existing cell ordering, suppression, and separator rules remain the source of
  truth. The uncached test path still renders thinking inline.
- Each section retains complete styled logical rows and a wrapped block.
  Only changed sections compare and wrap. Thinking continues to use the same
  four-row projection, including its heading and hidden-row summary.
- The frame borrows the materialized thinking source and captures duration
  once. Shared row joins retain old snapshots and indexed selection without
  flattening either static section.
- Empty visibility, width/theme changes, finalization, replacement, history
  dividers, and structural state updates preserve the cold-renderer contract.
  No public API, dependency, protocol, persistence, or snapshot changes are needed.

## Validation

Real pre-fix failures and the resulting bounds:

| Scenario | Before | After |
| --- | --- | --- |
| Initial paint plus 20 thinking appends | 21 assemblies | 1 |
| Wrapping during those 20 appends | 40,200 logical lines | At most 120 |
| One displayed-duration change | Reassembles the prefix | Retains it; wraps two fixture lines |

The growing-source fixture includes a 1,000-line prompt and 1,000 prior answer
paragraphs. Another work test adds 1,000 plan steps after thinking. Allocation
and selection checks assert that the prompt precedes thinking and the plan
suffix follows it, so both sides are actually exercised.

Cold-renderer comparisons cover empty/nonempty source replacements, styled
content, widths zero/one/eight/eighty, history separation, response transition,
finalization, and commit. They also cover a missing prompt/empty suffix and
rendering while source lines are already borrowed. The existing isolated theme
test now includes the dynamic thinking block. No snapshots were updated.

Cargo and the default Bazel TUI target each pass 973 cases, with three ignored
subprocess fixtures exercised by their parent tests. Strict workspace/all-target
Clippy, formatting, and diff checks pass. Bazel initially encountered incomplete
external module, crate, and toolchain caches. Targeted repository fetches
restored the default target without configuration changes; its final run took
72 seconds. The existing gold-linker deprecation warning remains unrelated.

```bash
cargo test --lib tui::
cargo clippy --workspace --all-targets -- -D warnings
bazel test //:rara_unit_tests --test_arg=tui::
cargo fmt --all -- --check
git diff --check
```

## Follow-Ups

Other structural input changes can still rebuild the prefix. Source Markdown
parsing, long selected rows, long mutable blocks, and source-wide reference or
sanitizer fallbacks retain their separate costs. Remote CI/review/merge and
physical-terminal acceptance remain independent gates. #921 stays open.
