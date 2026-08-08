# Runtime Extension Snapshot

## What changed

Added `RuntimeExtensionSnapshot` as the typed presentation projection for
extension counts, skill scopes, and agent status lines. `RuntimeClient` now
builds this projection from the session-owned registries and agent definition
cache. The runtime command processor passes it to the TUI snapshot reducer.

## Why

The TUI should display extension state without discovering hooks or agent
definitions itself. Moving the projection source into the runtime makes the
ownership boundary explicit and gives future app-server clients the same
snapshot contract.

## Trade-offs

The existing `TuiApp::sync_snapshot(&Agent)` entry point remains temporarily
for session restore and compatibility tests. It still contains the old local
projection path, so removing that entry point and the registry fields is the
next migration slice.

## Verification

- `cargo fmt --all`
- `cargo check --all-targets`
- Focused state snapshot coverage for applying a runtime-owned extension
  projection
