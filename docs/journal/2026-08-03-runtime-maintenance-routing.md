# Runtime Maintenance Routing Checkpoint

## What changed

Compact, backend rebuild, and DeepSeek/Kimi model-catalog operations now enter
the typed `RuntimeClientPort` command mux. This includes commands triggered by
slash commands, provider setup, model picker selections, profile changes, and
initial runtime startup.

The controller consumes `RuntimeMaintenanceCommand` values together with
runtime events, interactive input, and task completion. Existing in-process
task constructors and completion behavior remain unchanged in this slice.

## Why

These operations were the last major command families launched directly from
the terminal event path. Routing them through one command contract makes the
same behavior available to a future app-server client and keeps command order
observable at the session boundary.

## Remaining ownership work

The in-process controller still holds the session `RuntimeClient` and invokes
the compatibility task constructors after consuming commands. Moving those
constructors and runtime replacement into the command processor is the final
ownership step; it must preserve agent continuity, queued follow-ups, and
completion draining.

## Verification

- `cargo fmt`
- `cargo check --all-targets`
- `cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings`
- focused TUI command, controller, and runtime-port tests
