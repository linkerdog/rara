# Background Tasks Extraction

## Summary

Extracted background-task execution functions from `src/tools/bash.rs` into the
`rara-background-tasks` crate, completing the extraction started in the
previous commit (`fae99db`).

## Background

The initial extraction (`fae99db`) moved the background task tools
(BackgroundTaskListTool, BackgroundTaskStatusTool, BackgroundTaskStopTool),
the task store, records, status types, and output helpers. This left the
core execution functions (`run_background_bash_task`, `read_stream_chunks`,
`kill_child_process_group`) still in bash.rs.

## Scope

Moved from `src/tools/bash.rs` into `crates/rara-background-tasks`:

- `run_background_bash_task` — refactored to accept `sandbox_warning: Option<&str>`
  and `cleanup_path: Option<&Path>` instead of the full `WrappedCommand`, removing
  the sandbox-module dependency from the crate.
- `read_stream_chunks` — generic stream reader, also used by the non-background
  (synchronous) bash execution path.
- `kill_child_process_group` — best-effort process group cancellation, also used
  in the synchronous path.

The caller `spawn_background_bash_task` stays in bash.rs as glue. It computes the
sandbox warning string and cleanup path from `WrappedCommand` before calling
the crate's `run_background_bash_task`.

## Key Decisions

- `kill_child_process_group` and `read_stream_chunks` are `pub` in the crate so
  the non-background path in bash.rs can call them directly.
- Added `tokio` features `process`, `rt`, `macros` and `libc` (unix-only) to the
  crate deps for the moved execution code.
- Kept `spawn_background_bash_task` in bash.rs as the adapter between
  `WrappedCommand` and the crate's simplified interface.

## Validation

```bash
cargo check              # passes, only pre-existing warnings
cargo test -p rara-background-tasks  # 5/5 passed
cargo fmt                # clean
```

## File Sizes

| File | Before (phase 1) | Before (phase 2) | After |
|------|-----------------|-------------------|-------|
| `src/tools/bash.rs` | 2315 | 1971 | 1859 |
| `crates/rara-background-tasks/src/lib.rs` | — | 463 | 581 |

bash.rs is still over the 800-line threshold and needs further splitting by
tool group (file-split milestone tracked in `docs/todo.md`).
