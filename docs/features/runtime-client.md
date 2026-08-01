# Session Runtime Client

## Contract

RARA owns one runtime execution graph per interactive session. The graph
contains the `Agent`, goal handle, MCP and LSP managers, hook and skill
registries, prompt registries, sandbox state, and the runtime event bus.

`RuntimeClient` is the session-scoped ownership boundary for that graph. The
CLI constructs it from `RuntimeBootstrap` and passes the client to the TUI.
The TUI must not receive the bootstrap parts as independent arguments or
construct a second registry set.

The intended surface is:

- TUI submits typed runtime commands through the client;
- runtime execution publishes typed events and snapshots;
- TUI keeps only presentation state and local interaction state;
- runtime completion, continuation, rebuild, and persistence remain runtime
  responsibilities.

## Current Migration Slice

The first slice establishes session ownership and removes the wide bootstrap
argument list from `run_tui`. `TuiMaintainer` owns the client while `TuiApp`
continues to hold compatibility projections used by the existing command and
renderer modules. Those projections are not authoritative runtime state and
must be removed as command handling moves behind the client boundary.

Runtime task events are now awaited directly in the TUI event loop. A runtime
event wakes the loop immediately; the 166 ms timer remains only for periodic
UI work such as autoscroll, shared-task polling, and heartbeat display.

The lifecycle slice is now runtime-owned as well. Goal token accounting,
completion evaluation, continuation prompt construction, plan continuation
decisions, rebuilt-agent continuity merging, and runtime persistence helpers
live under `runtime_client` / `runtime_goal`. TUI completion code consumes
these typed outcomes and only applies transcript, overlay, phase, and queued
input presentation effects.

## Ownership Rules

1. A session has one `RuntimeClient` and one runtime registry graph.
2. TUI code may retain immutable handles needed to render a compatibility
   projection, but it must not discover plugins, register hooks, rebuild the
   runtime, or mutate the runtime registry directly.
3. Runtime commands and events must carry session identity when they cross a
   transport boundary.
4. Completion logic belongs to the runtime command processor. TUI completion
   handling is a migration compatibility layer and must not become a second
   goal or plan state machine. Goal and plan decisions are runtime-owned;
   presentation effects remain TUI-owned.
5. Snapshot hydration and live events must have an explicit ordering contract;
   a late snapshot must not overwrite newer live state.

## Follow-up

- Replace compatibility projections in `TuiApp` with a typed runtime snapshot.
- Move query, approval, compact, rebuild, and model-list actions to typed
  `RuntimeCommand` submission.
- Publish a TUI-facing typed projection event instead of converting
  `AgentEvent` into role/message strings and parsing those strings again.
- Move goal continuation, plan completion, agent replacement, and queued
  execution into the runtime command processor. Goal/plan decisions and
  rebuilt-agent continuity are now runtime-owned; queued command submission
  still uses the compatibility task bridge.
