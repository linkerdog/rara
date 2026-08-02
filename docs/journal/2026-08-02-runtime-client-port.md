# Runtime Client Port And TUI Controller Boundary

## What Changed

The TUI maintainer boundary is now named `TuiController`. A narrow
`RuntimeClientPort` contract defines typed runtime commands, snapshots,
projection events, completion, and transport lifecycle notifications. The
contract does not expose agents, registries, or task join handles.

The existing in-process runtime and compatibility task bridge remain unchanged
in this slice. This keeps production behavior stable while giving the future
app-server adapter and test fake a concrete seam to implement.

## Why

The TUI needs a replaceable runtime boundary before a scripted fake and shared
test harness can be added. Defining the contract first prevents the harness
from reproducing the current `Agent` and `RunningTask` coupling.

## Verification

- `cargo fmt --all`
- `cargo check --all-targets`
- Contract test verifies a fake captures typed commands without runtime objects.

## Remaining Work

- Route the production controller through `RuntimeClientPort`.
- Move snapshot projection out of `TuiApp::sync_snapshot(&Agent)`.
- Add `FakeRuntimeClient` and `TuiHarness` with a Ratatui `TestBackend`.
