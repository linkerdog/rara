# Runtime Feedback And Pending Decisions

## Problem

Running work, queued text, pending decisions, and completed output must remain
distinguishable. An accepted command is not proof that a turn stopped or a
decision finished executing.

## Scope And Non-Goals

This document owns visible terminal behavior. Authorization, task execution,
goals, persistence, and model switching remain runtime/feature contracts.
It does not introduce new control-plane messages or concurrency guarantees.

## Architecture

User intents cross `RuntimeClientPort` as typed commands. Runtime snapshots and
events update presentation state through the controller and TUI projection.
The renderer consumes the projection; it does not discover providers or own
runtime extension registries.

## Contracts

### RUN-01: Submission And Queueing

| State | User action | Visible/runtime outcome |
| --- | --- | --- |
| Idle, no pending decision | Submit ordinary text | Start or enqueue runtime input; clear the submitted composer |
| Running turn | Submit ordinary text | Queue a follow-up; keep the current turn and its progress visible |
| Running turn | Submit a slash command | Reject with a running-task notice, except quit aliases |
| Empty/whitespace input | Enter | No task; lightweight Ready feedback may be shown |
| Pending decision | Choose a displayed option | Send the corresponding typed decision; do not submit the option as a new task |
| Runtime rebuilding | Submit ordinary text | Preserve queued input until the runtime can accept it |

Queue preview and approval information may coexist. A queue indicator must not
hide a pending decision. Queued input is not rendered as a completed response.

### RUN-02: Cancellation Is A Transition

Ctrl+C with no overlay requests cancellation of a running turn. Esc does the
same unless it is handling the shell-approval rejection action in RUN-03. The
cancel command, cancellation-requested notice, terminal runtime event, and
task completion are separate states. Do not show successful completion merely
because a cancellation request was sent.

The controller coordinates terminal events with query completion so trailing
events are not lost. A task join failure must surface rather than leave the
presentation waiting indefinitely for an event that can no longer arrive.

### RUN-03: Approval Focus And Scope

Pending interaction priority is plan approval, shell approval, then requested
input. The visible card, option count, and keyboard mapping must agree.

- Plan approval has three decisions: approve, keep planning, and reject.
- Shell approval has four decisions in visible order: once, reusable prefix,
  always, and reject/suggestion. With no overlay, unmodified Esc selects
  rejection even when the composer contains a draft.
- Empty-composer arrows move selection; Enter applies it. Numeric shortcuts
  select explicit displayed options. Shell approval also accepts F1-F4.
- Nonempty text remains composer input instead of activating navigation letters.
- Request-input cards expose their offered choices and allow the supported
  free-text answer path. Do not imply Enter accepts a default when the request
  contract requires an explicit answer.
- Queue state, stale completed cards, or an unrelated overlay must not grant
  authorization. Approval scope is owned by the runtime policy.

See [planning mode](../features/planning-mode.md),
[shell approval](../features/shell-approval-policy.md), and
[thread goals](../features/thread-goals.md) for the underlying decisions.

### RUN-04: Transcript And Recovery

Render live progress, committed turns, tool lifecycle, thinking visibility,
and pending decisions from typed presentation state. Keep event chronology and
session identity intact. A disconnect is observable; a reconnect alone does
not prove that missed events or the active turn have been restored.

Resume behavior is owned by [threads](../features/threads.md) and
[session transcript](../features/session-transcript.md). Tests must distinguish
restored committed output, restored pending state, and newly running work.

## Validation Matrix

| Contract | Existing proving surface |
| --- | --- |
| RUN-01 | Busy-submit tests, queued-input tests, queue/approval render tests |
| RUN-02 | Controller completion-barrier tests and scripted runtime cancellation |
| RUN-03 | Pending-input dispatch, permission-mode tests, approval card render tests |
| RUN-04 | `TuiHarness` lifecycle tests, runtime event projection tests, transcript restore tests |

## Open Risks

- The scripted harness proves reducer/render behavior, not OS terminal key
  encoding, clipboard integration, or PTY restoration.
- New reconnect, event-ordering, or approval changes require targeted event
  sequences; existing happy-path coverage is not a universal safety proof.
- Busy-time command availability and queue-vs-steering semantics need an
  explicit product decision before new shortcuts are exposed.

## Source Journals

- [TUI interaction contracts](../journal/2026-09-17-tui-interaction-contracts.md)
- [TUI test harness](../journal/2026-08-02-tui-test-harness.md)
- [Goal resume and permissions](../journal/2026-09-16-goal-resume-permission-tui.md)
