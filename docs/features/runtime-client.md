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
argument list from `run_tui`. `TuiController` owns the client while `TuiApp`
continues to hold compatibility projections used by the existing command and
renderer modules. Those projections are not authoritative runtime state and
must be removed as command handling moves behind the client boundary.

Runtime task events and task completion are now awaited by one event-mux branch
in the TUI event loop. A runtime event wakes the loop immediately, and a closed
runtime receiver is drained through the task completion future instead of
falling back to receiver polling. The 166 ms timer remains only for periodic UI
work such as autoscroll, shared-task polling, and heartbeat display; it marks
the screen dirty only when one of those operations changes state.

Runtime-to-TUI task delivery uses `RuntimeControlEvent` directly. The TUI
matches `RuntimeEvent` variants for assistant, tool, memory, todo, warning,
and error behavior; it does not infer those semantics from transcript roles or
formatted message text. `Transcript` remains only for presentation-only
progress such as OAuth and model download messages.

The controller boundary now has a narrow `RuntimeClientPort` contract. It
exchanges typed commands, runtime projection events, snapshots, completion,
and transport lifecycle notifications without exposing `Agent`, registries, or
task join handles. The current in-process controller still uses the
compatibility task bridge behind this boundary; an app-server client and test
fake can be introduced without making those runtime objects part of the TUI
contract.

The lifecycle slice is now runtime-owned as well. Goal token accounting,
completion evaluation, continuation prompt construction, plan continuation
decisions, rebuilt-agent continuity merging, and runtime persistence helpers
live under `runtime_client` / `runtime_goal`. TUI completion code consumes
these typed outcomes and only applies transcript, overlay, phase, and queued
input presentation effects.

Extension status projection now has a typed `RuntimeExtensionSnapshot`. The
runtime command processor computes extension counts, scopes, and agent status
lines from the session runtime and passes the projection to the TUI snapshot
reducer. The TUI snapshot reducer no longer discovers extension state from an
agent; restore paths use the same runtime-owned projection helper.

Production TUI state no longer stores prompt, skill, or hook registry handles.
The runtime command processor owns these registries and passes cloned
`RuntimeTaskServices` to query, approval, and completion task construction.

## TUI Test Contract

TUI tests use the same runtime projection reducer and renderer as production.
`src/tui/testing` provides a test-only `FakeRuntimeClient` and `TuiHarness`;
the fake records typed commands and scripts snapshots, runtime events, turn
completion, cancellation, disconnect, and reconnect without constructing an
`Agent`, registries, task handles, or a real terminal.

The harness consumes `RuntimeProjectionEvent` through the same stream shape as
the future app-server client. Rendering uses the existing custom terminal with
an in-memory Ratatui backend adapter, so lifecycle tests do not create a second
rendering path. Script actions are awaited directly; tests must not use sleeps
to establish ordering.

The current harness is intentionally a deterministic projection/lifecycle
fixture. Virtual time, production `TuiController` construction from a port,
and full command routing remain follow-up slices.

The in-process TUI now has a `RuntimeClientPort` adapter backed by the
session's structured `RuntimeEventBus`. `TuiController` consumes that stream
for runtime projections and snapshots; the old task receiver is retained only
to observe completion and is drained without replaying events. This prevents
the same runtime event from being applied once through the port and again
through the compatibility channel.

The in-process adapter now owns a typed command channel. User prompts and
session cancellation enter the same event-loop mux as runtime projections and
task completion, so production TUI input no longer invokes those operations
directly from the terminal event branch. Plan, shell, and pending-input
approval responses use the same path. Compact, rebuild, and model-catalog
commands use the same path as well. The command processor still delegates
execution to the existing in-process task bridge. `TuiController` no longer
owns `RuntimeClient` or an `Agent`; an independent runtime command processor
owns task construction, completion, and runtime replacement access.

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

- Remove mutable extension-registry projections from `TuiApp`; all registry
  discovery and reload should remain runtime-owned while TUI consumes typed
  snapshots. Production task construction now receives explicit runtime-owned
  services; test-only fixtures retain local setup helpers.
- Publish a TUI-facing typed projection event instead of converting
  `AgentEvent` into role/message strings and parsing those strings again.
- Move goal continuation, plan completion, agent replacement, and queued
  execution into the runtime command processor. Goal/plan decisions and
  rebuilt-agent continuity are now runtime-owned; queued command submission
  still uses the compatibility task bridge.
