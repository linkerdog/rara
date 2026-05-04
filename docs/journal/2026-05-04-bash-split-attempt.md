# 2026-05-04: bash.rs split attempt (deferred)

## Goal

Split `src/tools/bash.rs` (2255 lines) into sub-modules per the
~800-line guideline.

## Attempted splits

| Sub-module | Content | Lines | Status |
|---|---|---|---|
| `bash/background.rs` | BackgroundTaskStore impl + 3 tool impls | ~420 | Failed |
| `bash/input.rs` | BashCommandInput + validation | ~300 | Deferred |

## Why it failed

`BackgroundTaskStore` and `BackgroundTaskRecord` types are tightly
coupled with the main `BashTool` — `spawn_background_bash_task`,
`run_background_bash_task`, and several helper functions access
record fields directly. Moving the implementation would require:

1. Making all `BackgroundTaskRecord` fields `pub` (or `pub(crate)`),
   which changes the API surface for all callers.
2. Moving `read_output_tail` / `append_background_output` helpers,
   which are shared between BackgroundTaskStatusTool and BashTool.
3. Updating `runtime_context/tooling.rs` imports.

Multiple attempts at line-range deletion (sed, apply_patch,
replace_lines) caused cascading errors because function boundaries
are unclear and type coupling is dense.

## Resolution

Deferred. The file remains at 2255 lines. The test module at
`tool_result.rs` (977-1362) was successfully extracted instead (→979
lines).

## Next steps

If bash.rs is revisited, consider:
- Extract `BackgroundTaskStore` + impl as a separate non-child
  module (e.g. `src/background_task.rs`) to avoid circular imports.
- Move only `read_output_tail` and `append_background_output`
  (simpler, ~60 lines).
- Attack `bash/input.rs` first (BashCommandInput is more isolated).
