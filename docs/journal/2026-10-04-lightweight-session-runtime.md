# Lightweight Session Runtime

## Summary

Extract `rara-runtime` with the real session actor, bounded commands, turn
ownership, stop fencing, shutdown receipts, event ordering, and replay. The
native application delegates lifecycle control through `NativeSessionDriver`;
the host builder injects its backend, tools, context, and transcript. Both use
the shared loop/model/tool effects rather than a separate host execution loop.

## Decisions

Moving the full native dependency graph would preserve the downstream failure.
Keeping a second minimal actor would make cancellation and replay diverge.
`SessionDriver` instead supplies the execution object, native policy controls,
pending-input payload, cleanup, and event projection to one owner.

The execution object returns before pending-input publication, memory capture,
terminal events, or another turn. Native child admission closes before shutdown,
and cleanup receipts outlive any caller awaiting them. The shared `EventLog`
retains receipt ordering and the existing replay/close boundary. Native public
methods and wire payloads remain compatible through re-exports and adapters.

Reference review used Codex revision
`f959e7fc9832dfa0ebfb6542ab1bbf829638ac24`, `core/src/session/turn.rs`, for
scoped cancellation and completion cleanup, and Claude Code revision
`4b9d30f7953273e567a18eb819f4eddd45fcc877`, `src/bridge/sessionRunner.ts` and
`src/services/tools/toolOrchestration.ts`, for session execution ownership and
serial result updates. The adaptation keeps transport and application policy
outside the shared ownership mechanism.

Host tools own authorization and may await host interaction. The host package
does not discover native plugins, load provider credentials, or write local
memory/transcript files. Host context is explicit and keeps its system prefix
stable. Full native context/provider extraction remains separate work.

Completed host tool replies enter history immediately, including partial
batches. A new resume regression caught an unanswered later call after
cancellation (two result blocks where the next request required three). The
existing native tool-pairing and history repair routines now live in the shared
agent crate and repair that abandoned call before the next prompt. Matching
results stay unchanged, and abandoned calls are never executed during recovery.

Model observation types moved to `rara-core` without changing their fields.
The runtime's native dependencies are target-scoped; browser builds receive a
single explicit unsupported-session-executor diagnostic, while core and agent
remain browser-compilable. This does not claim browser runtime execution.

## Validation

- Public host tests cover deltas, identities, accounting, hydration/readback,
  provider and tool cancellation return barriers, partial results and resume,
  dropped shutdown waiters, replay exhaustion, stream draining, and failures.
- Native session/input/source/skill tests and embedding integration tests run
  through the shared actor. Event-bus tests cover concurrent ordering and
  receipt-before-broadcast behavior after the log extraction.
- `cargo test --locked -p rara-core -p rara-agent -p rara-runtime`
- `cargo test --locked --lib runtime_session::`
- `cargo test --locked --lib runtime_event_bus::`
- `cargo test --locked --test runtime_session --test embedded_runtime`
- Strict workspace Clippy, Cargo formatting, browser core/agent compilation,
  and the default Bazel host/native session targets.
- `scripts/check_downstream_runtime.py --rev <published-full-sha>` creates a
  fresh external Git consumer, audits the production dependency closure and
  revision identity, and runs the same public fixture. No patches or lockfile
  are copied. The PR records the published revision and final results.

## Follow-Ups

The [downstream contract](../features/downstream-runtime.md) defines the #860
acceptance gate and supported Cargo form. Shared browser effects and accounting
clocks are covered by [the browser checkpoint](2026-10-04-browser-agent-effects.md).
#871 still requires native provider/context extraction, browser HTTP/SSE,
session scheduling, and browser runtime tests. Existing TUI migration and
store/parity work remain in
[the active backlog](../todo.md).
