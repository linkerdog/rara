# OpenCode-Inspired TUI Session Projection

## What changed

RARA runtime tool events now carry an optional `call_id`. TUI transcript
entries retain a typed tool payload with the tool name, call ID, and lifecycle
status while continuing to persist the existing role/message fields.

## Why

OpenCode keeps session state and tool parts in the runtime/server projection and
lets the TUI render that projection. RARA previously exposed tool semantics to
the TUI primarily through role strings. The typed payload is the first
backward-compatible step toward the same boundary.

## Trade-offs

The current agent event model does not yet assign a shared ID to every tool
use/result pair, so the field is optional and existing producers remain
compatible. Persisted transcript records are intentionally unchanged. A later
change must add runtime-owned compaction records and complete tool identity
correlation before removing the compatibility role path.

## Verification

- `cargo fmt --all`
- `cargo check --all-targets`
- Focused runtime event tests cover tool payload construction and lifecycle
  status.

## Remaining work

- Add a typed session projection owned by the runtime.
- Emit and render compaction as a structured session event.
- Move completed-tool rendering from role matching to projection matching.
