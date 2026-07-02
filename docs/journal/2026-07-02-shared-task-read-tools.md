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

- Add revision or timestamp based stale-read checks for `task_update`.
- Add explicit owner claim semantics instead of treating owner as a plain field update.
- Propagate task-list IDs through team and subagent runtime state.
- Add watcher/TUI surfaces once mutation semantics exist.

## Task Create Follow-Up

The next slice added `task_create` as the first write-side task tool. It creates pending tasks with no owner and empty dependency lists, assigns the next numeric task id while holding a task-list `.lock` file, and writes the task JSON atomically. This keeps task creation compatible with the existing `task_list` and `task_get` read tools without introducing update, claim, or dependency mutation semantics yet.

## Task Update Follow-Up

This slice added `task_update` as the first mutation tool for existing shared tasks. The tool updates subject, description, `activeForm`, owner, status, metadata, dependency edges, and deletion while holding the task-list `.lock` file. Dependency updates maintain both sides of the `blocks` / `blockedBy` edge, and deletion removes stale references from remaining tasks.

The implementation intentionally keeps owner as a plain field update for now. It does not yet require a caller-supplied revision or last-read timestamp, so concurrent claim/update conflict prevention remains follow-up work before this can be treated as a full multi-agent coordination primitive.

## Shared Coordination Follow-Up

This slice closes the multi-agent coordination gaps left by the first mutation pass:

- `task_update` now accepts `expectedRevision` / `expected_revision` and rejects stale writes with the current revision and timestamp.
- Stored task files now carry `revision` and `updatedAt`; successful non-delete updates bump both fields.
- `claimOwner` / `claim_owner` claims unowned tasks and rejects conflicting owners. `releaseOwner` / `release_owner` only clears ownership when the caller matches the stored owner.
- Runtime tool construction propagates one default task-list ID into parent tools, team tools, and subagent tool managers so ordinary coordination does not require repeating `taskListId`.
- General subagents now expose shared task tools but still do not expose repository, shell, browser, editing, or recursive agent tools.
- `/status`, `/context`, and the wide sidebar expose shared task progress from the runtime snapshot.

The UI follows the existing RARA sidebar summary style after checking Claude Code and OpenCode task displays. Claude keeps expanded tasks in a low-noise region near the composer, while OpenCode shows a collapsible sidebar list only when unfinished work exists. RARA keeps the existing sidebar fallback rule and only shows `Shared Tasks` when there is no active plan, goal, or session-local todo list.

Remaining risk: shared task state refreshes through runtime snapshots. There is still no live filesystem watcher or TUI command for switching the active shared task list during a session.
