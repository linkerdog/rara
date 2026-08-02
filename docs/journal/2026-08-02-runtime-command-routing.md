# Runtime Command Routing Checkpoint

## What changed

The in-process `RuntimeClientPort` now has a typed command channel. Production
TUI prompt submission and current-turn cancellation are sent through that port
and consumed by the same `tokio::select!` mux that handles runtime events and
task completion.

The command processor delegates to the existing runtime task bridge for this
slice. Test-only direct dispatch helpers remain available for focused TUI
tests, but the production terminal event loop no longer invokes them for
prompt or cancellation commands.

## Why

This removes a second production command path and makes command ordering
observable at the runtime boundary without prematurely moving every task
constructor and registry owner in one change.

## Trade-offs

Approval, compact, rebuild, and model-list commands still use the compatibility
bridge. The next migration must move those command families behind the same
port before `TuiController` can stop holding the in-process runtime objects.

## Verification

- `cargo fmt`
- `cargo check --all-targets`
- `cargo test --bin rara tui::controller --no-fail-fast`
- `cargo test --bin rara tui::runtime_port --no-fail-fast`
