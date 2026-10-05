# Portable Model Turn Checkpoint

## Summary

Extract model dispatch and response collection into
`rara-agent::execute_model_turn`. The application and external consumer use the
same implementation beneath the shared loop executor. The host provides a
prepared request and session-scoped `ModelTurnPolicy`; the effect returns the
assistant message, executable tool calls, stream/response evidence, and stop
reason.

## Background And Decisions

The shared loop executor removed duplicate scheduling, but model streaming and
response collection still belonged to the native application. Moving the
whole Agent would retain its context, persistence, tool, and presentation
dependencies. A second host loop would diverge from native execution.

Reference review used Codex revision
`f959e7fc9832dfa0ebfb6542ab1bbf829638ac24`: `core/src/session/turn.rs` captures
one prepared request view, while `tools/src/tool_executor.rs` separates a shared
execution contract from host policy. Claude Code revision
`4b9d30f7953273e567a18eb819f4eddd45fcc877`, `src/query.ts`, keeps continuation
and hook state under the existing turn owner. The adaptation extracts only the
model effect and retains native context/policy ownership.

`ModelTurnPolicy` observes success or failure before response processing, then
normalizes text, prepares tool arguments, and completes per-response policy.
Native accounting still finishes before propagating provider errors. Summary
prefix capture, usage updates, plan parsing/persistence, hook transformations,
and terminal output keep their existing order. Cancellation cleanup remains in
the enclosing asynchronous native loop adapter.

Tool call IDs and names remain provider-owned. The assistant history retains
original tool arguments even when hooks change executable arguments. Reasoning
metadata survives alongside visible content, but metadata-only responses do not
create assistant history. Text streams suppress fallback text emission. Policy
errors prevent subsequent blocks and completion effects.

The agent crate now depends on the existing portable core and uses serde_json
in production. It adds no runtime, transport, clock, filesystem, or provider
dependency. Native clocks remain in the accounting policy. Prepared message and
tool order is passed through unchanged, preserving cache-prefix behavior.

## Validation

- `cargo test --locked -p rara-agent`: model effect tests cover event order,
  fallback behavior, reasoning-only history, raw versus executable arguments,
  cancellation before dispatch, and provider/policy error boundaries.
- `cargo test --locked --lib agent::tests::`, plus
  `cargo test --locked --test runtime_session --test embedded_runtime`, with
  isolated `RARA_HOME`: native policy, transcript, plan, hook, and session
  regressions consume the shared model effect.
- Strict workspace/all-target Clippy, formatting, default Bazel agent/session
  targets, and browser-target checks verify integration.
- The external Git fixture uses both shared loop and model effects. The PR
  records the final published revision, native tests, browser compilation, and
  dependency-closure audit without workspace patches or a copied lockfile.

## Follow-Ups

Keep #860 and #871 open. Context preparation, native tool execution/policy, and
the real RuntimeSession actor still require extraction. A fake backend round
trip through shared effects is not the final session acceptance test. Browser
runtime execution and provider future/clock adaptation remain separate gates.
See the [canonical contract](../features/portable-agent-loop.md) and
[active work](../todo.md).
