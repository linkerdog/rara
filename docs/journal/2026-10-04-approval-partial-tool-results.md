# Partial Tool Results Across Approval Pauses

## Summary

Retain already completed tool results when a later call in the same batch
requests approval. The native loop appends those results before its existing
approval checkpoint, so paused transcript readback, persisted history, and the
next provider request keep the original evidence and provider call IDs.

## Background And Decisions

The shared batch returned completed messages together with `AwaitingApproval`,
but the native finalizer checkpointed history without consuming those messages.
The turn-local results were then dropped. This was inherited behavior, exposed
by auditing the session boundary after shared tool execution was extracted.

Codex revision `f959e7fc9832dfa0ebfb6542ab1bbf829638ac24`,
`codex-rs/core/src/session/turn.rs::drain_in_flight`, records completed tool
outputs in conversation history. Claude Code revision
`4b9d30f7953273e567a18eb819f4eddd45fcc877`,
`src/services/tools/toolOrchestration.ts::runToolsSerially`, yields each tool's
updates before admitting the next call. The local adaptation preserves completed
evidence at the existing approval boundary.

The ordinary batch commit also advances plan progress and adds a continuation,
so it is deliberately not reused for a pause. Approval still stops before the
pending call and later calls; no tool result is invented for unexecuted work.
The existing typed approval answer and session ownership stay unchanged.

## Validation

The regression uses `RuntimeSessionBuilder::for_host` with a scripted backend,
a custom evidence tool, and a recording shell that never executes commands.
Before the fix, paused readback contained zero completed result blocks instead
of one. The fixture covers both approval and rejection, enabled checkpoint
readback, original evidence in the next model request, provider call identity,
and no replay of completed or later calls.

- `cargo test --locked --lib runtime_session::input_tests::`
- `cargo test --locked --lib agent::tests::planning::`
- `cargo clippy --locked --workspace --all-targets -- -D warnings`
- `bazel test //:rara_unit_tests --test_arg=runtime_session::input_tests::`
- `cargo fmt --all -- --check`

## Follow-Ups

This fixes the approval boundary without changing public APIs or persistence
formats. Lightweight session extraction and browser runtime execution remain
the existing #860/#871 work in [the active backlog](../todo.md).
