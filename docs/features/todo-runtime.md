# Todo Runtime

## Problem

RARA has a first-class planning artifact, but it does not yet have a first-class execution checklist.
Today plan steps can be shown as progress, but that mixes two different concepts:

- the approved implementation plan, which is the user-facing agreement about scope and approach;
- the agent's current todo list, which is the mutable working set used while carrying out the task.

This makes it hard to preserve execution progress across long turns, compaction, approval pauses, and
session restore without polluting `plan.md` with transient status.

## Scope

- A session-scoped todo artifact for the current execution working set.
- A tool surface that lets the agent create, update, complete, and inspect todo items.
- Runtime and TUI state that can display todo progress when it changes and keep the active working
  set visible from status surfaces.
- Context assembly support so compacted or resumed turns can recover the current todo state.
- Clear separation between approved plan state and mutable todo state.

## Non-Goals

- Replacing `plan.md`.
- Treating every plan step as a todo item automatically.
- Building a project-level backlog manager.
- Syncing todos to GitHub issues, PR checklists, or external task systems.
- Using todo state as a permission or approval mechanism.

## Architecture

### 1) Session Artifact

Each interactive session may have a todo artifact beside the plan artifact:

- path: `.rara/sessions/<session_id>/todo.json`
- owner: session/runtime persistence
- lifecycle: created lazily when the agent first writes todo state
- scope: current session only

The artifact should be structured JSON rather than markdown so the runtime can merge, validate, and
render state without reparsing prose.

### 2) Todo Item Model

The minimum model is:

```json
{
  "version": 1,
  "items": [
    {
      "id": "todo-1",
      "content": "Inspect the command parser",
      "status": "in_progress",
      "updated_at": 1777584000
    }
  ],
  "updated_at": 1777584000
}
```

Valid item status values:

- `pending`
- `in_progress`
- `completed`
- `cancelled`

At most one item should be `in_progress` unless the runtime later gains explicit parallel-agent todo
ownership.

### 3) Tool Surface

RARA should expose a dedicated todo tool, aligned with Claude Code's `TodoWrite` concept but adapted
to RARA's runtime:

- tool name: `todo_write`
- input: complete replacement list of todo items
- output: normalized todo state and a concise summary of changes

The tool should update the whole todo list atomically rather than patching individual fields. This
keeps the model contract simple and makes it easy to validate status transitions.

The tool description and execute-mode prompt should also teach the working discipline around that
contract:

- use `todo_write` proactively for complex multi-step execution, not for trivial one-step work;
- resend the full working set when statuses, blockers, order, or validation work changes;
- keep at most one `in_progress` item unless multi-agent ownership exists later;
- prefer concrete execution items such as bug reproduction, focused tests, or final verification;
- do not treat implementation as effectively complete while the relevant verification item is still
  pending or failing.

A later `todo_read` tool is optional. In the first implementation, todo state can be provided through
runtime context and `/context`/`/status`, so a separate read tool is not required unless models need
an explicit recall action.

### 4) Runtime Context

Runtime continuation messages should include a compact todo snapshot when a todo artifact exists:

- total item count
- pending count
- in-progress item content
- completed count
- cancelled count

Compaction summaries must preserve active todo state under the existing `Plan State` or a dedicated
`Todo State` section. The compacted summary should not rewrite or replace `todo.json`; it only helps
the next model turn understand the state.

### 5) TUI Display

Todo progress should be displayed only when it changes or when explicitly requested from status
surfaces:

- `todo_write` results may render a compact `Todo Updated` card.
- the wide-screen sidebar may show a bounded `Todo` section with progress, active item, and a short
  checklist preview while todo state exists.
- `/context` should show the current todo artifact and active item.
- `/status` may show a brief count summary.
- Completed todo state should not stay pinned as a permanent bottom card after it becomes stale.

This keeps Todo visible as execution state without turning it into persistent transcript chrome.

## Contracts

### Plan vs Todo

- `plan.md` records the approved implementation plan and must not include todo progress.
- `todo.json` records mutable execution progress and must not replace plan approval.
- A plan approval can seed initial todos only if the agent explicitly calls `todo_write` after
  approval.
- Updating todos must not change plan approval state.
- Completing all todos does not imply the task is done unless the agent has also produced the final
  user-facing result.

### Persistence

- Todo writes must match the existing `SessionManager` atomic-write contract:
  write a unique temporary file next to `todo.json`, then replace the target
  with platform-aware replace semantics instead of relying on a plain
  rename-over-existing-file operation.
- Invalid todo state must be rejected with a structured tool error.
- Session restore must load the latest todo artifact if it exists.
- Missing todo files are valid and mean "no active todo state".

### Context Assembly

- Active todo state should enter context as structured runtime state, not as ad hoc prompt text.
- Todo state should have a visible source in `/context`, including the artifact path and update time.
- If todo state is too large for the active context budget, inject counts and the active item first,
  then omit completed items before omitting pending items.

### Execution Guidance

- The execute-mode prompt should tell the agent to keep todo state current for complex multi-step
  execution instead of tracking mutable progress only in prose.
- When a task changes non-trivial behavior, the todo list should normally preserve a focused
  reproduction, regression-test, or verification item until that validation has actually happened.
- Sidebar, `/status`, and `/context` should all project from the same structured todo state instead
  of keeping separate UI-only copies.

## Validation Matrix

- Unit tests for todo state validation and normalization.
- Session-manager tests for atomic save/load and missing-file behavior.
- Agent/runtime tests that `todo_write` updates the session artifact and continuation snapshot.
- TUI state/render tests for `Todo Updated` display and `/context` todo reporting.
- Restore tests that todo state survives session reload without mutating `plan.md`.

## Implementation Checkpoint

The first runtime slice implements Claude-style write semantics without automatic plan-to-todo
conversion:

- `todo_write` accepts a complete replacement list and normalizes item ids, statuses, timestamps,
  and the single-`in_progress` invariant.
- Successful writes update `Agent.todo_state`, atomically persist
  `.rara/sessions/<session_id>/todo.json`, and restore that state when the session is resumed.
- Runtime context exposes todo state as structured data so future `/context`, compaction, and
  protocol subscribers can consume the same source.
- TUI output hides the raw tool JSON and renders a compact `Todo Updated` transcript card only when
  the list changes.
- The runtime-control event stream emits a structured `todo.updated` event for Wire/ACP consumers
  instead of requiring them to parse transcript text.
- `/context` renders the current todo artifact path, counts, active item, and bounded item list from
  `TodoContextView`; `/status` renders a compact todo summary.

## Open Risks

- The first implementation should avoid automatic plan-to-todo conversion. Automatic conversion can
  produce noisy todo lists and may duplicate the model's own working set.
- Large multi-agent todo ownership is intentionally deferred until team/agent execution state has a
  clearer ownership model.
- The UI should avoid permanent todo cards; stale completed lists can become visual noise in long
  sessions.
- Sidebar todo previews must stay bounded so they remain useful instead of pushing out more stable
  context information.
- Future context compaction should decide whether to carry the active todo only or a bounded pending
  projection.

## Source Journals

- [2026-05-01-todo-and-review-specs](../journal/2026-05-01-todo-and-review-specs.md)
- [2026-05-03-todo-write-runtime](../journal/2026-05-03-todo-write-runtime.md)
- [2026-05-13-todo-verification-runtime-guidance](../journal/2026-05-13-todo-verification-runtime-guidance.md)
