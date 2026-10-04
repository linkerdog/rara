# Portable Tool Execution Checkpoint

## Summary

Move serial tool batch admission, invocation, completion, and provider-ID result
collection into `rara-agent::execute_tool_batch`. Native execution and the
independent host fixture use this implementation and `execute_tool_call`, which
binds trusted context and progress to the same provider call. The shared loop
consumes the batch's explicit approval outcome.

## Background And Decisions

Shared loop/model execution still left hosts to reproduce the tool batch loop
and result envelopes. A separate host implementation would diverge at approval,
error, and cancellation boundaries. Native permission/classifier policy,
hooks, inspection, todo/accounting effects, compaction, and batch budgets remain
in a session-owned adapter rather than becoming portable dependencies.

Codex revision `f959e7fc9832dfa0ebfb6542ab1bbf829638ac24`,
`tools/src/tool_executor.rs`, keeps invocation contracts separate from host
routing and policy. Claude Code revision
`4b9d30f7953273e567a18eb819f4eddd45fcc877`,
`services/tools/toolOrchestration.ts` and `toolExecution.ts`, awaits serial tool
execution and applies context updates before subsequent calls. This adaptation
retains existing serial ordering; it does not add concurrent tool execution.

`ToolBatchEffects` makes batch preparation, admission, invocation, and result
processing explicit. Approval pauses stop before the paused/later calls. Hard
policy failures preserve their original error and stop admission. Tool errors
remain inputs to result policy, allowing native model-visible error replies.
Successful replies omit `is_error`, preserving the existing transcript shape.

Native PreToolUse hook failures retain the existing explicit omission behavior;
diagnostics now use warning logging so they surface with the conversation.
Hook denial replies retain their prior event behavior. Native approval state
still determines pauses after restored interactions, and result compaction
finishes before admission of the next call. Cancellation is cooperative and
waits for actual invocation return; the session actor's terminal barrier stays
unchanged.

Approved shell resumes also use the shared invocation helper while retaining
their existing approval decision, checkpoint, and continuation path.

The helper binds call ID outside model arguments while preserving session,
turn, workspace, cancellation, and inference context. Native admission checks
the registry before emitting execution status; invocation looks up the same
tool again to avoid retaining a registry borrow across mutable host policy.
No new dependencies, runtime, spawning, transport, or clocks are introduced.

## Validation

- `cargo test --locked -p rara-agent`: suspended admission/invocation/completion,
  approval pauses, explicit omissions, error replies, hard policy errors,
  trusted identity, and cancellation cleanup before return.
- `cargo test --locked --lib agent::tests::`, plus
  `cargo test --locked --test runtime_session --test embedded_runtime`, with
  isolated `RARA_HOME`: native mode/approval/hook/result and session regressions.
- Strict workspace Clippy, Cargo formatting, browser-target core/agent checks,
  and default Bazel agent/session targets.
- The independent Git fixture now runs shared loop, model, and tool effects;
  tool replies use the native one-message-per-call envelope. The PR records the
  final remote revision, native/browser results, and dependency-closure audit.

## Follow-Ups

#860 and #871 remain open. Context/policy assembly and the real RuntimeSession
actor must still move into the lightweight host boundary, with existing
ownership, replay, and cancellation semantics. Shared effects alone do not
fulfill the public session acceptance gate or browser runtime execution.
See [the canonical contract](../features/portable-agent-loop.md) and
[active work](../todo.md).
