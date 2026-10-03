# Portable Agent Loop

## Problem

The application's agent loop mixes continuation decisions with model calls,
native hooks, tool execution, transcript persistence, and event publication.
Extracting an independent host loop would duplicate those decisions and allow
planning, cancellation, and completion semantics to diverge.

## Scope

`rara-agent` owns the existing loop's deterministic transitions. Its machine
accepts observations and completed-effect acknowledgements, then requests the
next effect. The machine performs no I/O, awaits no futures, and depends only
on serialization and error types. A shared asynchronous executor drives this
machine through the `LoopEffects` host interface. The executor owns effect
ordering and acknowledgements; the application supplies native effects.

The serializable control state includes the phase, continuation count, bounded
plan-repair and Stop-hook counters, pending response observations, and final
loop outcome. Public application execution modes re-export the shared type.

## Non-Goals

This checkpoint is not a complete serialized agent/session or a second host
runtime. The host still owns transcript content, in-flight model/tool results,
tools, cancellation tokens, permissions, context projection, hooks, stores, and
event publication. Restoring control state alone must not replay an external
effect or claim durable recovery. Store/effect coordination, provider adapters,
browser execution, and lightweight `RuntimeSession` packaging remain open in
[#860](https://github.com/linkerdog/rara/issues/860) and
[#871](https://github.com/linkerdog/rara/issues/871).

## Architecture

The application prepares a request, executes a model or tool effect, records
the resulting transcript/checkpoint, and acknowledges that completed effect.
The machine owns the next transition. A host must not acknowledge persistence
before its write succeeds or supply a completion for the wrong phase.

The principal boundaries are:

1. Check limits before preparing each model request.
2. Observe the model output before recording the assistant message; invalid
   plan-exit repair preserves the existing early-rejection behavior.
3. Record the assistant message before choosing tools or text continuation.
4. Consult Stop hooks only when a no-tool response needs no other continuation.
5. Wait for tool completion, then either pause for approval or commit results.
6. Commit continuation/results before admitting another model request.
7. Apply finalization before acknowledging the final loop outcome.

The root adapter contains effect handling, not an execution loop or a second
implementation of these decisions. Native approval/classifier/tool policy
remains in that adapter. Request construction and context projection retain
their existing owners and stable prompt order.

`execute_loop` awaits each host effect before acknowledging it to the machine.
`LoopEffects` implementations must complete required persistence or cleanup
before returning success. Failed effects return their original error without
acknowledgement, replay, another model request, or implicit finalization. The
shared progress value is updated before invoking an effect, including when the
effect later fails. Approval pauses retain their explicit finalization reason;
the adapter preserves the distinction from session-end cleanup.

The executor requires no async runtime, spawning facility, clock, filesystem, or
transport. It uses the existing Send-future convention; browser-specific future
bounds remain part of provider/transport work. Cancellation is cooperative:
hosts finish cancellation cleanup before returning an error. Dropping an
executor future does not imply cleanup or session completion.

## Contracts

### Model Effects

`execute_model_turn` is the shared model effect beneath `LoopEffects`. It
accepts the host's prepared message/tool view and turn metadata, checks
cooperative cancellation before dispatch, forwards stream events, and collects
the resulting assistant message, tool calls, and response evidence. It performs
no context assembly, clock reads, accounting, persistence, or tool execution.

`ModelTurnPolicy` observes the provider result before any response blocks are
processed. Hosts retain accounting and usage handling even when a request
fails. Policies may normalize text and prepare executable tool inputs; the
assistant message retains the provider's original tool arguments while each
executable call preserves its provider call ID. Native planning, hooks, and
terminal presentation remain policy effects in their original order.

Streamed text suppresses fallback text emission for that response. Reasoning
stream events and nonempty `reasoning_content` metadata both count as reasoning
evidence. Metadata is retained alongside visible text/tool content, but a
metadata-only response does not create an assistant history entry. Policy
failures stop collection before subsequent blocks or completion callbacks.

The application and downstream fixture consume this implementation. The shared
model effect is not an independent session runtime or a context/prompt policy.

### Tool Effects

`execute_tool_batch` owns serial admission, invocation, result completion, and
provider-ID result collection. `ToolBatchEffects` supplies session-scoped
policy: batch preparation, per-call admission, invocation, and result handling.
Every stage completes before the next call is admitted. An approval pause
retains completed results without synthesizing a result for the pending call.
After an approval answer has recorded its real result, native continuation
repairs any abandoned later calls before the next model request and checkpoint.
These synthetic results are errors; already completed calls are never replayed.
Cancel/interrupt during the pause uses the same repair before a fresh prompt.

An approval pause returns `AwaitingApproval` without invoking the paused call or later calls.
Effect errors propagate without replay or implicit transcript commit.

Admission may invoke a tool, provide an already-handled reply, pause for
approval, or explicitly omit a call after host diagnostics. The native adapter
preserves existing mode restrictions, approval/classifier/hook order, and its
legacy hook-failure omission. The shared executor does not grant permission.

`execute_tool_call` forwards the host's trusted context and binds progress and
the context call ID to the provider call. Model arguments cannot replace host
identity. Cancellation stays cooperative: invocation awaits the tool's actual
return and does not fabricate an early terminal result. Tool failures are
passed to host result policy, which may convert them into model-visible errors
as the native application does. Result-policy failures stop the batch.

`ToolReply` preserves the existing user-message tool-result envelope, omitting
`is_error` on success. Hosts retain result compaction, batch budgets, persistence,
and visible result events. The application and downstream fixture use the same
batch and invocation code.

### Loop Decisions

- `max_turns` wins over token exhaustion when both are reached. Limits are
  checked before model preparation, including after tool and continuation
  checkpoints. The existing counter counts tool iterations and forced
  continuations; a final no-tool response does not increment it.
- Invalid plan exit receives at most one repair continuation per loop entry.
  A second invalid exit stops without appending that assistant message.
- Plan-mode continuation retains shallow-plan, inspection-evidence, explicit
  inspection, and reasoning-only rules. Execute mode retains explicit
  inspection and reasoning-only continuation. Pending user input or shell
  approval suppresses those automatic text continuations. Review mode does not
  acquire Execute-mode continuation behavior.
- Stop hooks may block completion eight times per loop entry. A further block
  produces the existing diagnostic and allows completion. An earlier block
  remains active for later hook invocations within the same loop entry.
- Tool/plan approval pauses do not run `SessionEnd` hooks. Normal completion,
  hard limits, and exhausted plan repair retain their current hook behavior.
- Cancellation remains cooperative at the existing host/provider/tool boundary.
  This machine does not publish session terminal events or bypass the
  execution-return barrier in [runtime-session.md](runtime-session.md).
- Each effect carries a monotonically increasing identity within one machine
  instance. Hosts must pair it with the owning session and loop-entry identity;
  receipt numbers alone do not distinguish separate instances. Acknowledgements
  must return that identity; an old model/tool receipt cannot complete a newer
  request in the same phase. Invalid, duplicate, and out-of-order acknowledgements
  fail without advancing counters or changing the phase. Counter overflow is an
  explicit error.
- Serializing and restoring the control state at an acknowledged host boundary
  preserves its next transition. This is an internal versioned-code snapshot,
  with no cross-version persistence or external-effect replay guarantee.

## Downstream Use

Use `rara-agent` and `rara-core` from the same reviewed full Git revision and
normal default features. `scripts/check_downstream_core.py --rev <full-sha>`
creates a fresh consumer outside the workspace, without copying the repository
lockfile or patches. It audits the production closure of both crates, runs a
fake backend/custom-tool round trip driven by the shared executor, and compiles
the fixture for `wasm32-unknown-unknown`. The script requires that target to be
installed on the selected Rust toolchain. This validates control transitions
and contract composition; it does not exercise the application session API or
claim browser execution.

## Validation Matrix

| Boundary | Check |
|---|---|
| Transition guards | Invalid/duplicate completions leave the serialized machine unchanged |
| Limits | Zero/exact limits, competing limits, continuation counting, overflow |
| Continuation | Plan/Execute/Review, reasoning/text, inspection evidence, pending input |
| Bounded recovery | Plan repair and Stop-hook exhaustion retain exact counters and outcomes |
| Serialization | Round trips at each reachable pending effect preserve the next transition |
| Existing application | Agent planning, approval, hooks, duplicate-tool, budget, and session integration tests |
| Async effects | Suspended model/tool/checkpoint/finalization effects prevent later work; errors preserve progress and stop admission |
| Model effects | Stream/fallback ordering, reasoning evidence, original versus executable tool arguments, cancellation, policy errors, and metadata-only history |
| Tool effects | Approval pauses, suspended invocation/result handling, trusted call identity, error/omission replies, and serial result ordering |
| Dependency boundary | External Git fixture audits both core and agent dependency closures |
| Portable compilation | Native tests and browser-target compilation without feature flags |

## Implementation Sequence

1. Establish the pure machine and focused transition tests. Exit when invalid
   transitions, counter boundaries, and state round trips are demonstrated.
2. Establish the shared asynchronous executor and adapt existing native effects,
   retaining observable ordering. Exit when suspension/error tests and existing
   agent/session regressions pass without a second application driver.
3. Exercise the executor as a pinned remote Git dependency with the same public
   contracts used by the application. Exit after native/browser checks, strict
   Clippy, Cargo formatting, and default Bazel verification.

## Open Risks

Control-state serialization is narrower than durable agent recovery. Hosts
must pair it with their own transcript and effect ledger. The application still
owns native execution facilities; this crate alone does not make the existing
`RuntimeSessionBuilder` lightweight. Extracting model/tool effects and session
assembly must keep these decisions and preserve host authority instead of
growing another loop.

Session actor extraction begins after these gates and must preserve the
existing ownership and cancellation-return barriers.

## Source Journals

- [2026-10-04-portable-agent-loop](../journal/2026-10-04-portable-agent-loop.md)
- [2026-10-04-shared-agent-executor](../journal/2026-10-04-shared-agent-executor.md)
- [2026-10-04-portable-model-turn](../journal/2026-10-04-portable-model-turn.md)
- [2026-10-04-portable-tool-execution](../journal/2026-10-04-portable-tool-execution.md)
