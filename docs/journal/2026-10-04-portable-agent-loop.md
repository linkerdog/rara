# Portable Agent Loop Checkpoint

## Summary

Extract the existing application's deterministic loop decisions into
`rara-agent`. The root agent drives its effects through the existing native
model, tool, hook, checkpoint, and event implementations. Execution mode remains
available at its previous application path through a direct re-export.

## Background And Decisions

Issues #860 and #871 require a shared executor with a portable dependency graph.
The preceding contract extraction removed native dependencies from the LLM/tool
interfaces, but the execution decisions still lived inside the application.
This checkpoint extracts those decisions before moving host/session ownership,
so an external integration cannot acquire an independent simplified loop.

Reference review used Codex revision
`f959e7fc9832dfa0ebfb6542ab1bbf829638ac24`, `core/src/session/turn.rs`, for
follow-up admission, Stop-hook feedback, and cancellation boundaries. Claude
Code revision `4b9d30f7953273e567a18eb819f4eddd45fcc877`, `src/query.ts`, makes
continuation state explicit and retains bounded recovery state across hook
continuations. The adaptation keeps this repository's existing limits and
event/checkpoint order rather than importing different product policy.

The machine performs no I/O or async work. It serializes phases, observations,
counters, and issued effect identity. Every acknowledgement must match both
phase and the current identity; a delayed tool/model completion cannot advance
a newer request. Identities are scoped to a machine, so the host also retains
session/turn ownership. Finalization requires its own acknowledgement.

The native driver synchronizes the outer query's continuation count before
executing each effect, retaining progress when a later effect fails. Model
interrupts retain the existing SessionEnd path, while approval pauses omit it.
No runtime prompt order, transcript schema, or session terminal barrier changes.

The external Git fixture now resolves both agent and core from one immutable
revision, audits both dependency closures, and drives a fake backend/custom
tool cycle through the same public machine. This is control-state composition,
not the complete RuntimeSession acceptance gate.

## Validation

- `cargo test --locked -p rara-agent`: 12 tests covering phase/receipt guards,
  model/tool stale receipts, serialization through pending phases, continuation
  modes, bounded recovery, approval pauses, limits, and counter overflow.
- `cargo test --locked --lib agent::tests::` with an isolated temporary
  `RARA_HOME`: 153 passed, one pre-existing ignored test. Four pure predicate
  tests moved into the machine suite; one new native test verifies assistant
  checkpoint before tool execution, result checkpoint before the next request,
  and progress retention after a later backend failure.
- `cargo test --locked --test runtime_session --test embedded_runtime`: all
  nine integration tests pass, including cancellation, replay, and concurrency.
- Strict workspace/all-target Clippy, Cargo formatting, and browser-target
  core/agent compilation pass. A temporary path-consumer proof passes both
  fixture tests and browser-target test compilation; the final gate uses a
  published Git revision rather than this path proof.
- Default Bazel agent, filtered application agent, runtime-session, and embedded
  targets pass. Missing external caches were restored with targeted fetches;
  no Bazel configuration changed. The generated lock adds the new crate edges.
- Record the published Git revision and fresh downstream outcome in PR evidence.

## Follow-Ups

Keep #860 and #871 open. Full executor effects, policy adapters, lightweight
session packaging, browser runtime/transport/accounting support, and remaining
workspace migration gates are still required. Restoring this control state
does not restore a transcript, replay effects safely, or provide durable agent
recovery. The [canonical spec](../features/portable-agent-loop.md) and
[TODO](../todo.md) define those boundaries.
