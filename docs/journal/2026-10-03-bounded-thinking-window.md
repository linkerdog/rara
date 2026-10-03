# Bounded Thinking Window

## Summary And Scope

This is a focused #921 follow-up to the shared source and response-row caches.
Live thinking displayed four trailing logical rows but cloned its entire
materialized Markdown body first. The cell now borrows that body, selects the
window, and copies only visible span contents. The collapsed committed preview,
duration, hidden-row count, dim styles, and transcript ordering are unchanged.

## References And Plan

Inspected local implementations before editing:

- Codex `f959e7fc9832dfa0ebfb6542ab1bbf829638ac24`, `chatwidget.rs`:
  active-cell revision and animation keys keep the render-only tail current.
- Claude Code `4b9d30f7953273e567a18eb819f4eddd45fcc877`, `Markdown.tsx`,
  `VirtualMessageList.tsx`, and `AssistantThinkingMessage.tsx`: stable/mutable
  Markdown separation, immutable-message lookup, and compact thinking surfaces.

The plan isolates visible-row projection first, with production-path work-count
regressions, then audits the outstanding table-interruption boundary. General
active-prefix caching needs exhaustive mutation tracking; this change does not
introduce an incomplete cache key or freeze other cards. Local edits, normal
stacked delivery, Cargo, and default Bazel checks require no new protocol,
schema, dependency, or configuration contract.

## Key Decisions

- A private content enum separates owned committed messages from borrowed live
  rows. Live construction always selects the existing four-row tail. It cannot
  accidentally concatenate a full message with a cloned stream anymore.
- Head and tail projection share the same span-styling loop. Selection happens
  before copying; hidden rows are neither visited nor cloned by this cell.
- Test-only counters are per stream and are attached at the actual cell copy
  boundary through normal app rendering. They count copied body rows, excluding
  chrome, Markdown parsing, general prefix assembly/comparison, wrapping, and
  terminal output. A selected row can itself contain arbitrarily many bytes.
- Tables interrupting mutable paragraphs already preserve canonical preceding
  prose. An exhaustive two-chunk split check now covers plain and multiline
  prose, Unicode/CRLF, and reference contexts, then compares final styled rows
  with the complete-message renderer. No table production change was needed.

## Validation

The instrumented pre-fix implementation reproduced two behavioral failures:

| Production scenario | Before | After |
| --- | --- | --- |
| 20 unchanged paints after 32 thinking rows | 740 copied rows | 80 |
| 200 line appends and paints | 21,900 copied rows | 800 |

The unchanged-frame regression also exercises 4,096 source rows. It checks that
repeated presentation does not parse the source again. Additional checks cover
complete span styles, empty rows, short/empty bodies, committed head/tail
projection, duration, and hint placement. Two initial expected-value mistakes
(empty-line spans and code-language-label indentation) were corrected; they
were fixture failures, not additional product regressions.

Cargo and the default Bazel target each pass the complete 957-test TUI suite;
three ignored subprocess fixtures are exercised by their parent tests. Strict
workspace/all-target Clippy, formatting, and diff checks pass. A scoped
dependency fetch recovered a missing external `rules_rust` package before Bazel
validation; no Bazel configuration changed. Existing snapshots are unchanged,
and every touched Rust source file remains below 1,000 lines.

Validation commands:

```bash
cargo test --lib tui::
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
bazel test //:rara_unit_tests --test_arg=tui::
git diff --check
```

## Follow-Ups

#921 remains open for general active-prefix/non-streaming/committed-thinking
assembly, comparison and changed-prefix rewrapping, long mutable Markdown and
source-wide reference replay, plus review/CI/merge and physical-terminal gates.
The four-row presentation bound does not imply bounded total per-delta work.
