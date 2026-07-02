# Shared Task Lists

## Problem

RARA has a session-scoped `todo_write` checklist for the current agent turn, but it does not yet have a shared task surface that can coordinate project work across subagents or future teams. Claude Code now treats `TaskCreate`, `TaskGet`, `TaskList`, and `TaskUpdate` as the durable task-list family while `TodoWrite` remains a session checklist fallback. RARA needs the same separation so session progress and shared backlog state do not drift into one artifact.

## Scope

- A workspace-local shared task store under `.rara/tasks`.
- Read-only `task_list` and `task_get` tools that expose Claude-compatible summary and detail shapes.
- A `task_create` tool that creates pending tasks safely in the shared task store.
- A `task_update` tool that updates task fields, status, metadata, dependencies, and deletions under the same task-list lock.
- Field compatibility for `blockedBy` and `activeForm` in stored task JSON.
- Documentation of the remaining stale-read and ownership follow-up work before enabling multiple agents to claim tasks concurrently.

## Non-Goals

- Stale-read protection, ownership conflict resolution, or task watchers.
- Replacing session-scoped `todo_write`.
- Syncing shared tasks to GitHub issues, external trackers, or transcript todos.

## Architecture

The shared task store is a file-backed workspace artifact:

```text
.rara/tasks/<task_list_id>/<task_id>.json
```

`task_list_id` is sanitized to an ASCII path segment and defaults to `default`. Each task is one JSON file so write-side tools can update individual tasks without rewriting a whole task list. `task_create` holds a per-task-list `.lock` file while assigning the next numeric task id and atomically writing the new task file. `task_update` uses the same lock while rewriting a task file, adding dependency edges, or deleting a task.
Because task creation and updates perform blocking file I/O and file locking, async tool wrappers must run store writes on a blocking worker instead of directly on the Tokio executor.

`task_id` is treated as an identifier, not a path. Read tools reject empty task IDs, absolute paths, directory separators, and parent-directory traversal fragments before joining with the task-list directory.
Task-list reads do not follow symlinks: task-list IDs must resolve to real directories, task files must be real files, and each task JSON `id` must match the `<task_id>.json` filename.

The runtime registers one shared `TaskListStore` during tool manager construction and passes it to the shared task tools. Missing task-list directories are valid and return an empty list.

Tool schemas expose both RARA snake_case `task_list_id` and Claude-compatible camelCase `taskListId`. Callers should send only one of the two names. `task_create` and `task_update` also expose both `activeForm` and `active_form`; sending both aliases in the same call is invalid.

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

`task_create` accepts:

```json
{
  "subject": "Implement shared task creation",
  "description": "Create pending tasks in the shared task store.",
  "activeForm": "Implementing shared task creation",
  "metadata": {
    "source": "agent"
  }
}
```

Created tasks always start as:

- `status`: `pending`
- `owner`: absent
- `blocks`: `[]`
- `blockedBy`: `[]`

`task_create` returns:

```json
{
  "task": {
    "id": "1",
    "subject": "Implement shared task creation"
  }
}
```

`task_update` accepts partial updates:

```json
{
  "taskId": "1",
  "status": "in_progress",
  "owner": "agent-a",
  "metadata": {
    "priority": "high",
    "obsolete": null
  },
  "addBlockedBy": ["2"]
}
```

The update contract is:

- `status` accepts `pending`, `in_progress`, `completed`, or `deleted`.
- `deleted` removes the task file and removes stale `blocks` / `blockedBy` references from remaining tasks.
- `subject`, `description`, `activeForm`, and `owner` replace the stored field. Empty `activeForm` or `owner` clears that optional field.
- `metadata` merges into the stored metadata map; a `null` value deletes that key.
- `addBlocks` and `addBlockedBy` add bidirectional dependency edges and deduplicate repeated task IDs.

`task_update` returns:

```json
{
  "success": true,
  "taskId": "1",
  "updatedFields": ["metadata", "owner", "status"],
  "statusChange": {
    "from": "pending",
    "to": "in_progress"
  }
}
```

Missing tasks return a non-fatal outcome:

```json
{
  "success": false,
  "taskId": "missing",
  "updatedFields": [],
  "error": "Task not found"
}
```

## Validation Matrix

- Store tests cover sorted file loading, `blockedBy` alias parsing, `activeForm` alias parsing, sanitized task-list IDs, and completed-blocker filtering.
- Store tests cover path-like `task_id` rejection and file-vs-directory task-list handling.
- Store tests cover symlink rejection for task-list directories and task files, plus JSON id and filename consistency.
- Store tests cover `task_create` id allocation, pending defaults, atomic file readability, and symlinked task-list rejection.
- Store tests cover `task_update` field changes, metadata merge/delete, status-change reporting, dependency edge updates, and deletion cleanup.
- Tool tests cover `task_create` output, strict schemas, empty required-field rejection, `task_update` status/owner/metadata changes, deleted status handling, alias conflict rejection, invalid dependency IDs, `task_list` summary output, `task_get` detail output, missing tasks, and invalid `task_id` rejection.
- Workspace checks should run `cargo test tasklist`, `cargo test tools::tasklist::tests`, `cargo check --locked --workspace --all-targets`, and `cargo clippy --locked --workspace --all-targets -- -D warnings`.

## Open Risks

- `task_update` still needs revision or timestamp based stale-read checks before multiple agents can safely claim or update tasks concurrently.
- Owner claim semantics are still only a field update; conflicting claims are not rejected yet.
- Team and subagent runtime state still needs a task-list-id propagation contract.
- TUI rendering for shared tasks is intentionally deferred until watcher and mutation semantics are stable.

## Source Journals

- `docs/journal/2026-07-02-shared-task-read-tools.md`
