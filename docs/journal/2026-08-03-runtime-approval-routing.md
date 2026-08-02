# Runtime Approval Routing Checkpoint

## What changed

Production TUI approval actions now submit typed `InputControlRequest` values
through `RuntimeClientPort`. This includes plan decisions with feedback, shell
approval scope, pending request-input answers, numeric approval shortcuts, and
the automatic shell approval triggered by full-access mode.

The controller consumes these commands from the same event-loop mux used for
runtime projections and task completion, then invokes the existing approval
task constructors through the runtime-owned agent slot.

## Why

Approval was the remaining interactive path that could bypass the runtime port
after prompt submission and cancellation had migrated. Keeping it on a single
typed command path makes ordering and transport replacement consistent without
changing approval semantics or task lifecycle behavior.

## Trade-offs

Compact, rebuild, and model-list commands still use the compatibility path.
The in-process controller still owns the runtime adapter and agent slot until
those command families are migrated.

## Verification

- `cargo fmt`
- `cargo check --all-targets`
- `cargo test --bin rara tui::submit --no-fail-fast`
- `cargo test --bin rara tui::input_control --no-fail-fast`
