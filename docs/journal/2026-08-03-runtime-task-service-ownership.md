# Runtime Task Service Ownership

## What changed

Added `RuntimeTaskServices` as a runtime-owned bundle for prompt, skill, and
hook registries. `RuntimeCommandProcessor` now supplies this bundle to query,
approval, queued follow-up, and rebuild completion paths. Production `TuiApp`
state no longer stores these registry handles.

Rebuild completion updates the processor-owned service bundle when a new
runtime is installed. Test-only compatibility fixtures keep local registry
fields so existing unit tests can assemble isolated task environments.

## Why

Registry discovery and execution are runtime responsibilities. Passing an
explicit service bundle keeps task construction tied to the session runtime
without making registry ownership part of the presentation model.

## Trade-offs

Legacy no-port task helpers remain available for focused tests and transitional
callers. Production event-loop command handling uses the processor-owned
bundle; the remaining legacy helpers are the next cleanup target.

## Verification

- `cargo fmt --all`
- `cargo check --all-targets`
- Focused TUI input-control and task lifecycle tests
