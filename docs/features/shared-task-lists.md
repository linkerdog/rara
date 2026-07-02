# Shared Task Lists

## Problem

RARA has a session-scoped `todo_write` checklist for the current agent turn, but it does not yet have a shared task surface that can coordinate project work across subagents or future teams. Claude Code now treats `TaskCreate`, `TaskGet`, `TaskList`, and `TaskUpdate` as the durable task-list family while `TodoWrite` remains a session checklist fallback. RARA needs the same separation so session progress and shared backlog state do not drift into one artifact.

## Scope

- A workspace-local shared task store under `.rara/tasks`.
- Read-only `task_list` and `task_get` tools that expose Claude-compatible summary and detail shapes.
- Field compatibility for `blockedBy` and `activeForm` in stored task JSON.
- Documentation of the write-side follow-up work before enabling task mutation.

## Non-Goals

- Implementing `task_create` or `task_update` in the first slice.
- Claiming, ownership conflict resolution, file locks, or task watchers.
- Replacing session-scoped `todo_write`.
- Syncing shared tasks to GitHub issues, external trackers, or transcript todos.

## Architecture

The shared task store is a file-backed workspace artifact:

```text
.rara/tasks/<task_list_id>/<task_id>.json
```

`task_list_id` is sanitized to an ASCII path segment and defaults to `default`. Each task is one JSON file so later write-side tools can update individual tasks without rewriting a whole task list. The first implementation is read-only and therefore does not need locks yet.

The runtime registers one shared `TaskListStore` during tool manager construction and passes it to both read tools. Missing task-list directories are valid and return an empty list.

## Contracts

Task files use this minimum shape:

```json
{
  "id": "1",
  "subject": "Implement shared task read tools",
  "description": "Expose task_list and task_get.",
  "activeForm": "Implementing shared task read tools",
  "owner": "agent-a",
  "status": "pending",
  "blocks": ["2"],
  "blockedBy": []
}
```

Valid status values are:

- `pending`
- `in_progress`
- `completed`

`task_list` returns:

```json
{
  "tasks": [
    {
      "id": "1",
      "subject": "Implement shared task read tools",
      "status": "pending",
      "owner": "agent-a",
      "blockedBy": []
    }
  ]
}
```

Completed blockers are filtered from `blockedBy` in summary output so available work can be found directly from `task_list`.

`task_get` returns full details:

```json
{
  "task": {
    "id": "1",
    "subject": "Implement shared task read tools",
    "description": "Expose task_list and task_get.",
    "status": "pending",
    "blocks": ["2"],
    "blockedBy": []
  }
}
```

Missing tasks return `{ "task": null }`.

## Validation Matrix

- Store tests cover sorted file loading, `blockedBy` alias parsing, `activeForm` alias parsing, sanitized task-list IDs, and completed-blocker filtering.
- Tool tests cover `task_list` summary output, `task_get` detail output, missing tasks, strict schemas, and empty `task_id` rejection.
- Workspace checks should run `cargo test tasklist`, `cargo test tools::tasklist::tests`, `cargo check --locked --workspace --all-targets`, and `cargo clippy --locked --workspace --all-targets -- -D warnings`.

## Open Risks

- Write-side tools need file locking, stale-read checks, and conflict handling before multiple agents can safely claim or update tasks.
- Team and subagent runtime state still needs a task-list-id propagation contract.
- TUI rendering for shared tasks is intentionally deferred until the write contract exists.

## Source Journals

- `docs/journal/2026-07-02-shared-task-read-tools.md`
