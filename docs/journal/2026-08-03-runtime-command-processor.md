# Runtime Command Processor Checkpoint

## What changed

`RuntimeClient` ownership moved out of `TuiController` into
`RuntimeCommandProcessor`. The event loop now owns the processor separately
from the presentation controller and passes it explicit commands, task
completions, and snapshot synchronization requests.

The processor owns agent access for query, approval, compaction, rebuild,
model-catalog, completion, and runtime replacement operations. The controller
now retains presentation state, the runtime port, and the event mux only.

## Why

Keeping the session runtime in the presentation controller made it too easy to
add execution or lifecycle behavior to TUI code. Separating the processor
establishes the ownership boundary needed by future app-server clients while
preserving the current in-process task implementation.

## Remaining boundary

`TuiApp` still contains compatibility projections for extension registries.
Those fields are populated by runtime bootstrap and are not discovered by the
TUI, but they should eventually be replaced by typed snapshot data.

## Verification

- `cargo fmt`
- `cargo check --all-targets`
- `cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings`
- focused controller, event-loop, command, input-control, submit, and port tests
