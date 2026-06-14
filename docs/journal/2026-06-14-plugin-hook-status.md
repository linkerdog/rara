# 2026-06-14 — Plugin Hook Runtime Status

RARA now reflects plugin hook registration in the TUI `/status` Extensions
panel. Runtime rebuild completes plugin hook registration before refreshing the
runtime snapshot, and the snapshot reads the in-process `HookRuntime` hook count
instead of relying only on repository file discovery.

This keeps the status panel aligned with the hooks that will actually dispatch
for the current session. File discovery remains as a fallback so existing
repo-local hook counts still render before runtime registration is available.
Plugin discovery and registration now run on a blocking worker during runtime
rebuild so synchronous filesystem work does not block the async runtime, and the
hook registry uses a synchronous lock because registry reads are not held across
await points.

Verification:

- `sync_snapshot_reports_registered_runtime_hooks`
- existing rebuild and status tests continue to cover local model/sidebar state

Remaining follow-up:

- Hook output injection into model context is still blocked on the sandbox
  policy decision tracked in `docs/todo.md`.
