# Shared Task Read Tools

## What Changed

- Added a workspace-local shared task store under `.rara/tasks/<task_list_id>/<task_id>.json`.
- Added read-only `task_list` and `task_get` tools.
- Matched Claude-compatible task field names in tool output, including `blockedBy`.
- Accepted stored `blockedBy` and `activeForm` aliases so existing Claude-style task JSON can be read directly.

## Why

Claude Code's public tools reference lists `TaskCreate`, `TaskGet`, `TaskList`, and `TaskUpdate` as the durable task-list tool family, with `TodoWrite` disabled by default in favor of those tools as of Claude Code 2.1.142. RARA already has a session-scoped `todo_write` checklist, so shared tasks should be modeled as a separate workspace artifact instead of stretching session todos into a multi-agent backlog.

The first slice is intentionally read-only. It lets agents inspect shared work without introducing file-locking, claim, or stale-update semantics before those contracts are designed.

## Trade-Offs

- The tool names use RARA snake_case (`task_list`, `task_get`) while output fields remain Claude-compatible (`blockedBy`).
- `task_list_id` is optional and defaults to `default`; explicit team/subagent task-list propagation remains follow-up work.
- Invalid task JSON is skipped with a warning during list reads, but `task_get` surfaces parse failures for the requested task.

## Remaining Work

- Add write-side `task_create` and `task_update` tools with locks, stale-read checks, and ownership semantics.
- Propagate task-list IDs through team and subagent runtime state.
- Add watcher/TUI surfaces once mutation semantics exist.
