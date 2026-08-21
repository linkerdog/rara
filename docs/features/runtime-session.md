# Runtime Session

## Problem

RARA currently assembles one agent runtime graph but exposes several independent
ownership paths. The TUI owns a `RuntimeClient`, the embedding facade owns an
`Agent`, ACP owns an `AcpSessionRuntime`, and Wire and print consumers own raw
agents. These paths duplicate lifecycle behavior and make cancellation,
replacement, persistence, and event ordering depend on the presentation or
transport that started a turn.

The root library facade is also too application-oriented for another Rust
application to use RARA as its agent runtime. Provider and runtime traits are
private, construction is configuration-driven, and embedding the root package
pulls presentation and provider implementations that a host may not need.

## Scope

- Define one public, session-scoped `RuntimeSession` handle.
- Keep all mutable agent and runtime state behind one `SessionActor`.
- Provide explicit dependency injection for backends, tools, transcript state,
  tool middleware, context policy, memory lifecycle, and event observation.
- Serialize commands for one session while allowing different sessions to run
  concurrently.
- Preserve ordered typed events, stable turn and tool-call identity, snapshots,
  cancellation, rebuild fencing, and explicit shutdown.
- Make TUI, embedded, ACP, Wire, print, and AppServer surfaces adapters over the
  same session API.
- Keep the library boundary usable by hosts such as Nowledge Mem that need to
  replace an existing agent/provider harness incrementally.

## Implementation Status

This document records both the delivered session boundary and its target
architecture. The 2026-08-22 checkpoint implements:

- the public `RuntimeSession`, `RuntimeSessionBuilder`, `RuntimeTurn`, and
  `RuntimeHost` APIs;
- explicit backend and tool injection, stable host session IDs, transcript
  hydration/snapshot/replacement, and configurable bounded queues;
- a host mode that disables ambient extension discovery, RARA memory
  retrieval/consolidation/capture, and local transcript, context, and
  compaction checkpoints;
- same-session serialization, cross-session concurrency, cancellation,
  ordered replay, provider call identity, rich terminal evidence, and explicit
  shutdown that closes and waits for the session's active child-agent tree;
- embedded, ask, print, exec, Wire, and ACP adapters over the session handle.

The following target items are not delivered by this checkpoint: TUI command
migration, agent rebuild and queued/steered input, store-trait injection,
lightweight crate extraction, a network AppServer transport, and the Nowledge
Mem parity harness. They remain explicit follow-up work and must not be treated
as production Rig-replacement evidence.

## Non-Goals

- Define a C ABI, FFI ABI, or WASM host API.
- Make transport connection identity the same as runtime session identity.
- Treat a dropped client connection as an implicit session shutdown.
- Add crash-retry semantics for an ambiguously dispatched provider request.
- Require a network AppServer for in-process Rust embedding.
- Make host-specific memory, tenancy, or authorization fields part of RARA's
  model-visible tool arguments.

## Architecture

### Compatibility Status

New library and headless integrations use `RuntimeSession`. `EmbeddedRuntime`
delegates to it, and ask, print, exec, Wire, and ACP are adapters over the same
handle. The TUI `RuntimeClient` remains a temporary compatibility owner while
its rebuild, approval, goal, and maintenance commands are moved behind the
session command boundary. It is not a second public embedding API.

### Target Library Layers

The stable dependency direction is:

```text
rara-core       ids, messages, events, usage, snapshots
rara-llm        backend and streaming contracts
rara-tools      tool contracts and call context
rara-agent      provider/tool agent loop
rara-runtime    RuntimeSession, SessionActor, RuntimeSessionBuilder
       ^
       +-- embedded compatibility facade
       +-- TUI adapter
       +-- ACP, Wire, and print adapters
       +-- RuntimeHost and AppServer
       +-- external Rust hosts
```

Application-owned provider implementations, TUI rendering, ACP, OAuth, and
local-model dependencies must not be unconditional dependencies of the minimal
runtime library.

### Session Ownership

`RuntimeSession` is a cloneable command and observation handle. It does not
expose `Agent`, registries, mutable runtime state, locks, or task join handles.

`SessionActor` is the only mutable owner of:

- the root `Agent` and agent-tree control;
- session identity, workspace, and state scope;
- the active turn and cancellation token;
- prompt, skill, hook, MCP, LSP, goal, and memory lifecycle handles;
- queued or steered input;
- runtime replacement generation;
- ordered event publication and session snapshots.

The actor processes commands through a bounded channel. When a model turn is
running, the agent is moved into an execution task and returned with the typed
turn outcome. The actor remains responsive to cancellation and follow-up
commands while that task is active.

### Runtime Host

`RuntimeHost` is an optional, injected multi-session owner. It stores session
handles by `RuntimeSessionId` and shuts independent sessions down concurrently.
It is not a process-global singleton. Generation-safe whole-agent replacement
is part of the target contract, not the current checkpoint.

TUI and a simple embedded application may own one session directly. ACP and
AppServer use a host because they can address multiple sessions. Transport
connection state, capabilities, subscriptions, and replay cursors remain owned
by the adapter or AppServer connection.

### Commands And State

Every mutating request carries a request identity and targets one session.
Prompt submission returns a `TurnId`. Commands return a typed acknowledgement
or a typed error such as `Busy`, `InvalidState`, `Unsupported`, `Overloaded`, or
`Closed`.

The target runtime state machine is:

```text
Initializing -> Idle
Idle -> Running -> Idle
Running -> Cancelling -> Idle
Running -> AwaitingInput -> Running|Idle
Idle -> Rebuilding -> Idle
Any live state -> Closing -> Closed
```

Invalid transitions are rejected explicitly. A stale task completion carries
the generation that created it and cannot install an older agent after a
rebuild.

Follow-up behavior is explicit. `Steer` is delivered at the next safe model
boundary; `Queue` waits for the current turn to finish. The runtime must not
silently reinterpret one mode as the other. Neither command is implemented in
the current checkpoint.

### Events And Snapshots

Each event envelope contains:

- `session_id`;
- `turn_id` when the event belongs to a turn;
- `agent_id` when the event belongs to a child agent;
- a monotonically increasing per-session `sequence`;
- a stable `event_id`;
- typed event data.

Tool start, progress, result, and failure events carry the provider tool-call
identity from the source. Adapters must not reconstruct identity by tool name.

The session assigns sequence numbers whether or not a subscriber is present.
Sequence allocation, replay insertion, and live publication share one ordered
critical section so concurrent event producers cannot publish sequence `N + 1`
before `N`. A subscription begins from an atomically captured snapshot and
sequence. A bounded replay gap produces `ResyncRequired`; lag must not be
discarded silently. After shutdown, an observer drains every event already
published to its stream before receiving the typed `Closed` boundary.

Thinking, assistant output, and tool lifecycle events for a turn precede its
terminal event because the actor publishes that boundary only after the root
execution callback returns. Diagnosing events from future externally managed
asynchronous producers after `TurnCompleted`, `TurnCancelled`, or `TurnFailed`
remains target work.

### Embedding And Host Injection

`RuntimeSessionBuilder` has one assembly path for CLI and library hosts. The
RARA application adapter supplies configuration discovery, default extensions,
provider construction, and local state. A custom host supplies explicit
components and does not trigger ambient plugin, credential, or memory discovery
unless requested.

`RuntimeSessionBuilder::for_host` requires the backend and exact `ToolManager`
up front and rejects construction without an explicit `with_state_root`. A host
may set a stable identity with `with_session_id`, hydrate prior messages with
`with_transcript`, and read the atomic completed transcript from
`RuntimeTurnOutcome`. It may also use `transcript` and `replace_transcript`
while the session is idle. Local transcript, context, and compaction
checkpoints, extension discovery, and memory facilities are disabled by default
on this path; each can be opted into explicitly. The host remains responsible
for durable commit and authorization.

Cancellation and execution errors retain the same `RuntimeTurnOutcome`
evidence as successful turns through `RuntimeSessionError::turn_outcome`. This
keeps partial transcript and usage data available after a terminal failure.

The current public backend request supports messages, tool definitions,
context-budget and cache-profile metadata, cooperative cancellation, streaming
text and reasoning deltas, and final-response tool calls and usage. Typed
output constraints, per-request provider options, and streaming tool-call and
usage events are not yet public; they remain required Nowledge Mem parity work.

The public tool contract carries trusted session, turn, call, workspace, and
cancellation context. Host tool implementations can own approval, budgeting,
safety filtering, audit behavior, and authority rather than accepting those
values from model arguments. A distinct injectable middleware stack remains
target work.

Direct transcript handoff, usage observation, and memory opt-out are
implemented. Async transcript/context store traits remain target policy seams.
An embedding host may explicitly disable RARA's default memory integration;
this is required when the host is itself the memory system.

`EmbeddedRuntime` remains a compatibility facade during migration and delegates
to `RuntimeSession`. It is not a second runtime owner.

## Contracts

1. Exactly one mutable agent owner exists for a runtime session.
2. No public runtime API contains TUI state or Ratatui types.
3. Same-session mutation is serialized; different sessions may execute
   concurrently.
4. Cancellation does not wait for the running agent to return to an idle lock.
5. Event sequence and terminal ordering are assigned by one session-owned
   publication boundary.
6. Snapshot hydration cannot overwrite a later live event.
7. Runtime replacement must be generation-fenced when it is added.
8. Adapters translate commands and events but do not implement turn lifecycle.
9. Memory is an injected facility, not a runtime or session owner.
10. The target minimal Rust library does not require TUI, ACP, local-model,
    OAuth, or application provider implementations.
11. Dropping one handle or transport connection does not stop a registry-owned
    session.
12. Shutdown drains or diagnoses memory and persistence failures rather than
    silently discarding them.

## Validation Matrix

| Area | Status | Validation |
| --- | --- | --- |
| State machine | partial | Focused tests cover idle, running, cancelling, closing, and representative invalid transitions; awaiting-input and rebuild states are target work. |
| Serialization | delivered | Two commands for one session never run two root turns concurrently. |
| Concurrency | delivered | Two sessions can block at the provider boundary and make progress independently. |
| Cancellation | delivered | A cooperative provider receives cancellation without waiting for the agent task lock; completion occurs when the backend observes the token or otherwise returns. |
| Replacement | target | A completion from an older generation must not replace the rebuilt agent after rebuild support is added. |
| Event order | delivered | Concurrent producers preserve increasing sequence values; thinking, text, and tool events precede the terminal event. |
| Replay | delivered | Snapshot plus replay has no gap; an exhausted replay window returns `ResyncRequired`, and shutdown drains published events before `Closed`. |
| Tool identity | delivered | Repeated same-name calls retain distinct provider call IDs. |
| Adapters | partial | Embedded, ACP, Wire, print, exec, and ask use `RuntimeSession`; TUI command ownership remains compatible but separate. |
| Isolation | delivered | Workspace, state root, MCP, LSP, hooks, memory, and child-agent controls remain session-scoped. |
| Library | partial | An integration fixture injects a fake backend, tool, stable identity, and transcript; async store traits remain target work. |
| Dependency boundary | target | The future minimal runtime dependency graph excludes Ratatui, Candle, ACP, and OAuth. |
| Build | partial | Cargo formatting, checks, Clippy, and tests pass; the default Bazel configuration is blocked before analysis by an unsupported local startup option. |

## Host Integration Example

```rust
let session = RuntimeSessionBuilder::for_host(config, workspace, backend, tools)
    .with_state_root(state_root)
    .with_session_id(host_session_id)
    .with_transcript(previous_messages)
    .build()
    .await?;

let outcome = session
    .query_with_events(prompt, AgentOutputMode::Silent, consume_event)
    .await?;
host_store.commit(&outcome.transcript, &outcome.query_report).await?;
session.shutdown().await?;
```

The host backend translates RARA `Message` values and tool schemas to the
chosen provider API. Host tools capture tenant authority in their Rust object
and use trusted `ToolCallContext`; tenant identity must not be accepted from
model-generated tool arguments.

## Operational Notes

- Command and event queues are bounded. Overload is a typed result.
- Cancellation is cooperative. A host backend must observe
  `LlmTurnMetadata` in a context-aware request method; cancellation
  acknowledgement is immediate and propagates to active children, while turn
  completion and shutdown still wait for root and child provider calls to
  return.
- Provider retry must not replay observable text or a completed tool side
  effect.
- AppServer applies separate bounds to connection ingress and outbound queues.
- A transport reconnect resumes from an event cursor or requests a new
  snapshot; it does not recreate the session implicitly.

## Open Risks

- Extracting the current root agent loop into a lightweight crate requires
  dependency inversion for provider construction, hooks, persistence, and
  extension discovery.
- Existing persisted transcripts need an explicit compatibility codec before a
  host changes message formats.
- Durable prompt admission and crash continuation require a separate contract
  for provider-dispatch ambiguity.
- Provider parity must be demonstrated independently for OpenAI Chat,
  Responses, Anthropic Messages, Gemini, and subscription OAuth paths.

## Source Journals

- `docs/journal/2026-08-22-runtime-session.md`
