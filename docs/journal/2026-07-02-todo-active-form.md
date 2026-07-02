# Todo Active Form

## Summary

RARA now accepts Claude-compatible `activeForm` labels in `todo_write` input
and stores them as `active_form` on session todo items. The active todo summary
uses `active_form` while preserving `content` as the stable imperative checklist
label.

## Background

Claude Code's compact `TodoWrite` guidance distinguishes:

- `content`: imperative work item shown in the checklist;
- `activeForm`: present-continuous label shown while the item is in progress.

RARA already had session-local `todo_write` persistence and display, but it only
tracked `content` and `status`.

## Scope

- Add optional `active_form` to `TodoItem`.
- Accept both `activeForm` and `active_form` in `todo_write` input.
- Require `activeForm` in the advertised tool schema for new model calls while
  keeping old persisted JSON and direct normalizer callers compatible.
- Keep shared `TaskList` / `TaskGet` task-store support as a separate follow-up.

## Validation

```bash
cargo test todo::tests -- --nocapture
cargo test tools::todo::tests -- --nocapture
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets -- -D warnings
```

## Follow-Ups

- Add a shared `.rara/tasks/<task_list_id>/` store and read-only TaskList /
  TaskGet tools for team/subagent coordination.
