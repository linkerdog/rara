# Runtime Session Checkpoint

## Summary

RARA now exposes one public `RuntimeSession` command and observation handle for
library and headless integrations. A private session actor is the sole mutable
owner of the root agent between turns, accepts bounded commands, leases the
agent to one execution task at a time, and remains responsive to cancellation.

`EmbeddedRuntime`, ask, print, exec, Wire, and ACP now delegate to this handle.
ACP uses a non-global `RuntimeHost` for its independent sessions. The TUI keeps
its existing `RuntimeClient` compatibility owner until its broader rebuild,
approval, goal, and maintenance command set is moved into the actor.

## Background

The previous embedding and protocol surfaces each took ownership of a raw
`Agent`. That duplicated lifecycle behavior, made cancellation depend on
surface-specific locks, and prevented an external Rust host from injecting its
own backend and tools without also adopting RARA's ambient runtime discovery.

## Scope

- Added `RuntimeSessionBuilder::for_host` for explicit backend and tool
  injection.
- Disabled ambient plugin, hook, skill, agent, and MCP discovery on the host
  path, and disabled RARA-owned memory capture by default there.
- Added typed session and turn IDs, lifecycle snapshots, turn outcomes, busy
  and overload errors, cancellation, and explicit shutdown.
- Added stable host session IDs and transcript hydration, idle replacement,
  snapshots, and completed-turn handoff. Host mode disables internal transcript
  history, context, and compaction checkpoints and requires an explicit state
  root so the embedding application remains the durable owner without falling
  back to process-global state.
- Disabled RARA memory retrieval, consolidation, and built-in capture on the
  host path instead of treating the memory system as another runtime owner.
- Added a bounded per-session event replay window with explicit
  `ResyncRequired` gaps and a typed `Closed` boundary after buffered events
  have been drained.
- Propagated provider tool-call IDs through agent events, tool context,
  structured runtime events, exec JSONL, Wire, and ACP.
- Added `RuntimeHost` for independent multi-session ownership without a
  process-global registry and concurrent host shutdown.
- Preserved ACP's protocol session ID as the canonical runtime and host ID
  instead of generating a second adapter-local identity.
- Made root cancellation propagate through the session-owned child-agent tree;
  shutdown closes child admission and waits for active child tasks to return.
- Split touched oversized inline test and plan-parser sources so every changed
  Rust source file remains below the repository's 1000-line limit.

## Key Decisions

- The actor owns mutation; cloned handles only submit commands or observe
  snapshots and events.
- One session rejects a second concurrent root turn with `Busy`; different
  sessions execute concurrently.
- Cancellation sets the active turn token inside the actor and never waits for
  the leased agent to return behind a presentation lock. Provider interruption
  remains cooperative: the same signal reaches active children, and completion
  waits for each backend to observe its token or otherwise return.
- Event sequence allocation, replay insertion, and live publication share one
  ordered critical section and continue even when no subscriber is attached.
  Concurrent producers cannot publish `N + 1` before `N`; thinking, assistant,
  and tool events are published before the terminal session event. Closing a
  session cannot hide a terminal event that is already buffered for an
  observer.
- Provider call IDs are authoritative. Adapters no longer reconstruct tool
  identity by tool name.
- Memory remains a facility owned by the session graph. A memory-system host
  can disable RARA capture instead of creating a second session owner.
- Success, cancellation, and execution failure preserve the same turn evidence;
  partial transcript and usage do not disappear on a terminal error.

## Validation

```bash
cargo fmt --all -- --check
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
cargo test --lib
cargo test --test runtime_session
cargo test --test embedded_runtime
cargo test runtime_event_bus::tests
cargo test -p rara-tools tool::tests
cargo test acp::tests
```

The runtime-session integration tests cover injected backend and tool
execution, trusted session/turn/call context, same-session serialization,
lock-independent cancellation, terminal event ordering, provider call-ID
preservation, host-owned transcript handoff, idempotent shutdown, failure
evidence, closed-stream draining, child-tree shutdown, concurrent event
publication, ambient agent-definition isolation, ACP identity continuity, and
concurrent progress across two hosted sessions.

The repository now pins Bazel 9.2.0 through `.bazelversion`, which is also the
version selected by Bazelisk in CI. The default Bazel invocation is currently
blocked before analysis by the local user bazelrc passing
`--experimental-disk-cache-gc-max-size=200G`, which Bazel 9.2.0 rejects as an
unknown startup option. The new integration target is wired in `BUILD.bazel`;
it still needs execution in an environment whose default Bazel configuration
is valid.

## Follow-Ups

- Move the TUI compatibility processor's rebuild, approval, goal, and
  maintenance commands behind `RuntimeSession`.
- Extract the agent loop and session actor into lightweight crates that do not
  unconditionally depend on TUI, Candle, ACP, OAuth, or application providers.
- Add injectable transcript and context stores and a Nowledge Mem compatibility
  harness before replacing Rig in production paths.
- Keep the `rara-app-server` crate as the protocol contract until a concrete
  transport is required; this checkpoint does not add a network server.
