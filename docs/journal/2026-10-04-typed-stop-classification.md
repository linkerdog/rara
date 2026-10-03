# Typed Stop Classification

## Summary

Remove the remaining error-substring cancellation classification in the TUI
compatibility bridge. A query uses its admitted `QueryStopKind`; maintenance
tasks without stop admission retain errors as diagnostics. Provider error text
can no longer choose the query's terminal presentation or hide a compact error.

## Background And Decisions

The #942 re-review asked whether the remaining `cancelled by user` substring
match was intentional. Two focused regressions demonstrated the mismatch: the
real query path published `TurnFailed` but completion selected the cancelled
Idle state, and the maintenance helper converted the error into a finished
event. Neither path had accepted a stop request.

Codex revision `f959e7fc9832dfa0ebfb6542ab1bbf829638ac24` uses typed
`TurnAbortReason` in `codex-rs/tui/src/chatwidget.rs`. Claude Code revision
`4b9d30f7953273e567a18eb819f4eddd45fcc877` checks the abort signal and
`AbortError` in `src/services/tools/toolExecution.ts`. The adaptation uses the
existing typed local stop owner rather than introducing another error parser.

Accepted stop ordering, cooperative execution return, preserved provider error
diagnostics, and queued-follow-up policy remain unchanged. The cancellation
follow-up fixture now supplies an admitted typed stop instead of treating an
error string as authority. Public APIs and event payloads are unchanged.

## Validation

- `cargo test --locked --lib unrequested_cancellation_text` reproduces both
  pre-fix failures and protects maintenance and real query/controller paths.
- `cargo test --locked --lib tui::runtime::` covers stop arbitration, queued
  follow-ups, approval cleanup, permissions, and completion handling.
- `bazel test //:rara_unit_tests --test_arg=tui::runtime::`
- Cargo formatting, Clippy, and whitespace checks.

## Follow-Ups

Physical terminal and multiplexer acceptance remain the existing delivery
work for #922/#925. This fix addresses the remaining source-level outcome
classification question from #942 review.
