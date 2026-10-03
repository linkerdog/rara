# Shared Agent Executor Checkpoint

## Summary

Move asynchronous loop scheduling and machine acknowledgements into
`rara-agent::execute_loop`. Both the application and the external Git fixture
use that executor. The native application implements the session-scoped
`LoopEffects` interface for model, assistant, continuation, hook, tool, result
commit, and finalization work; it no longer owns a second scheduling loop.

## Background And Decisions

The pure machine checkpoint established shared decisions, but each consumer
still had to drive its effects manually. A host could accidentally acknowledge
a checkpoint before its write completed or return before cleanup. The executor
now awaits every effect before acknowledging it. Any effect failure stops
admission and returns the original error without replay or implicit cleanup.
Progress is published before the effect, preserving outer recovery accounting.

Reference review used Codex revision
`f959e7fc9832dfa0ebfb6542ab1bbf829638ac24`, `tools/src/tool_executor.rs` and
`core/src/session/turn.rs`: reusable execution contracts coexist with host
policy and session-owned effects; hook feedback is recorded before follow-up.
Claude Code revision `4b9d30f7953273e567a18eb819f4eddd45fcc877`, `src/query.ts`,
retains explicit loop state and recovery guards across asynchronous work.
The adaptation keeps existing application policies and event/checkpoint order.

The executor uses the existing async-trait/anyhow contracts and adds no runtime,
spawning, transport, filesystem, or clock dependency. The machine itself stays
pure and serializable. Send-future compatibility remains unchanged; actual
browser execution and provider future/clock adaptation are separate work.
The object-safe effect interface allows later session assembly to own one
adapter without encoding native policy in its type. Boxed effect futures are
the trade-off; moving the entire native Agent would retain its dependency graph.

Native approval pauses still omit SessionEnd cleanup. Model interruption still
awaits native cancellation cleanup before returning. The outer session actor's
execution-return barrier is unchanged. Dropping a future does not imply that
cleanup completed, and control snapshots do not restore host-owned effects.

## Validation

- `cargo test --locked -p rara-agent`: machine regressions plus runtime-free
  polling tests for every suspended effect, failures at every stage, retained
  progress and error identity, approval finalization, checkpoint failure, and
  budget changes at result commit.
- `cargo test --locked --lib agent::tests::` with isolated `RARA_HOME`, plus
  `cargo test --locked --test runtime_session --test embedded_runtime`: existing
  native policy, transcript, hook, cancellation, and session regressions.
- The external Git consumer now calls `execute_loop` with a fake backend,
  custom tools, and a host-owned transcript. It checks deltas, trusted call
  identity, ordered readback, cooperative cancellation, and the pure snapshot
  API. Manual machine stepping no longer stands in for execution.
- Strict workspace Clippy, formatting, default Bazel, browser-target compile,
  and final fresh Git dependency results are recorded in the PR evidence.

## Follow-Ups

Keep #860 and #871 open. The existing RuntimeSession builder still enters native
application assembly. Move portable model/tool effects and session ownership
next, preserving one executor and the established terminal/cancellation
barriers. The final host acceptance test must use RuntimeSession, not only this
executor. See the [canonical spec](../features/portable-agent-loop.md) and
[active work](../todo.md).
