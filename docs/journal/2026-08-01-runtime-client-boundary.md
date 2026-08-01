# Runtime Client Boundary

## What changed

Added a session-scoped `RuntimeClient` created from `RuntimeBootstrap`. It owns
the session Agent and runtime handles for goal, MCP, LSP, hooks, skills,
prompts, sandbox state, and the event bus. `run_tui` now receives one client
instead of a long list of independent runtime objects.

`TuiMaintainer` owns the client alongside presentation state. Agent task
events are awaited directly from the event loop's `tokio::select!`; the timer
is no longer the mechanism that wakes the TUI for agent output.

## Why

Passing bootstrap parts directly to a presentation surface made ownership
ambiguous and allowed TUI state to become a second runtime orchestrator. The
client creates one explicit session boundary while keeping the existing
runtime behavior available during migration.

## Trade-offs

The current slice retains compatibility projections in `TuiApp` and legacy
task completion code. They are still transitional and are not the target
architecture. Moving them requires typed command and snapshot contracts so
goal and plan state do not get duplicated between runtime and TUI.

## Verification

- `cargo fmt --all`
- `cargo check --all-targets`
- `cargo test --bin rara tui::runtime::events --no-fail-fast`

## Remaining work

Command submission and completion orchestration still use the compatibility
task bridge. The lifecycle portion is now separated: runtime code owns goal
and plan decisions, rebuild continuity, and persistence helpers, while TUI
code applies only presentation effects. The next slice should move command
submission and queued execution behind typed runtime commands, then replace
the role/message compatibility path with typed runtime projection events and
snapshots.
