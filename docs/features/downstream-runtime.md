# Downstream Runtime

## Problem

Depending on the application package pulls native providers, presentation,
authentication, and repository-specific dependency patches into a host that
already supplies its own model backend and tools. Cargo does not propagate a
dependency's workspace patches to an external consumer.

## Scope

`rara-runtime` provides a native Tokio session builder, actor, typed event log,
replay, transcript access, and cooperative cancellation. It depends on the
shared `rara-agent` executor and `rara-core` model/tool contracts. It has no
application-provider, TUI, ACP, OAuth, local-model, plugin, or persistence
dependency.

The application's existing `rara::RuntimeSession` remains a compatibility
adapter with its current public methods, configuration, and event payloads. Its
session ownership, lifecycle transitions, event ordering, and replay now use
the same implementation as `rara_runtime::RuntimeSession`. Both execution
adapters use `rara_agent::execute_loop`, model dispatch, and tool invocation.

## Non-Goals

- Export application configuration or built-in native tools through the host package.
- Discover ambient extensions, read project instructions, or choose a provider.
- Persist transcripts, resume in-flight effects after a crash, or own host memory.
- Provide native shell/plan approval UI in the host adapter. Injected tools own
  permission checks and await host interaction before returning; the actor
  continues to observe cooperative cancellation while they wait.
- Claim browser runtime execution. The package rejects browser targets with a
  focused diagnostic; `rara-core` and `rara-agent` remain browser-compilable.
  Browser clocks, future bounds, transport, and session scheduling are #871 work.

## Architecture

`SessionHandle` and its actor are the only turn owners. An admitted turn transfers
its executor into the execution task. Cancel/interrupt only signal it; terminal
events and completion receipts wait for the real future to return. An adapter
restores its executor before the actor exposes completion or accepts another turn.

`SessionDriver` supplies scoped policy, resources, application controls, pending
interaction payloads, and event projections. The native adapter retains
approval, child-agent shutdown, memory capture, extension controls, and
persistence. The host adapter owns only the injected backend/tools and transcript.
Neither adapter implements another scheduling loop.

`EventLog` serializes sequence allocation, bounded replay retention, receipt
retention, and broadcast. Snapshot subscriptions subscribe live before capturing
their cursor, deduplicate replay/live overlap, recover available broadcast lag,
and require resynchronization when the cursor has expired. Closing drains
already published events before the stream returns `Closed`.

## Contracts

The host builder requires an absolute workspace path and rejects blank explicit
session IDs. It never changes cwd or reads/writes a state root. Each tool receives
trusted session, turn, provider call, workspace, cancellation, and accounting
context separately from model arguments. Tool errors become model-visible
results. Completed results are recorded before admitting another tool, retaining
partial evidence when a later call fails or cancellation is observed.
Before a new prompt is admitted to model execution, the shared transcript
repair routine adds interrupted-result errors for abandoned calls and retains
real completed results. This is the same pairing repair used by the native
application; it never executes abandoned work or reports it as successful.

The optional host system prompt is prepended in a stable position to each model
request. Transcript hydration and readback contain conversation history, not
that system prompt. Hosts own context budgeting and persistence and can replace
the transcript while idle. Provider errors and cancellation retain partial
transcript and query-report evidence in `RuntimeSessionError::turn_outcome`.

## Supported Dependency And Version Policy

Use the host package directly; the root application package still includes its
native adapters. Pin all project packages to the same full, published Git SHA:

```toml
[dependencies]
rara-runtime = { git = "https://github.com/linkerdog/rara.git", rev = "<full-40-character-published-sha>" }
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

The placeholder must be replaced by an actual revision. PR descriptions record
the exact revision accepted by the fresh downstream fixture. A moving branch,
the repository's development package version, or an unverified tag is not an
equivalent compatibility guarantee. Do not copy workspace patches or Cargo.lock.

```rust
use rara_runtime::RuntimeSessionBuilder;

let session = RuntimeSessionBuilder::for_host(workspace, backend, tools)
    .with_session_id(host_session_id)
    .with_system_prompt(host_instructions)
    .with_transcript(previous_messages)
    .build()
    .await?;
let subscription = session.subscribe_from_snapshot()?;
let turn = session.submit("Summarize this article").await?;
let outcome = turn.wait().await?;
host_store.commit(&outcome.transcript).await?;
session.shutdown().await?;
```

Consume `subscription.events` concurrently to receive model deltas and tool
progress during execution. `cancel_turn` fences cancellation to an observed
turn; a stop receipt acknowledges admission, while `turn.wait()` proves cleanup
has returned. Cloned session handles share the same owner and shutdown result.

## Validation Matrix

| Contract | Evidence |
| --- | --- |
| Independent package | `scripts/check_downstream_runtime.py --rev <full-sha>` creates a fresh external manifest/lockfile and resolves the published Git dependency |
| Dependency boundary | The verifier audits the runtime production dependency closure against a bounded allowlist and verifies all project packages use one revision |
| Public session | The same `crates/rara-runtime/tests/host.rs` fixture runs locally and unchanged in the external consumer |
| Model/tool evidence | Streamed deltas, system-prefix stability, repeated tool names, trusted identity, accounting, hydration, and transcript readback |
| Stop barrier | Provider and tool gates remain pending after cancellation until explicitly released; completed partial results survive |
| Shutdown/replay | Cancelled shutdown waiter, durable repeated shutdown, exhausted cursors, and drain-before-closed |
| Native parity | Existing session/input/source/skill, event ordering, embedding, and shutdown tests exercise the shared owner |
| Browser boundary | Core/agent compile without feature flags; runtime reports its unsupported browser scheduler rather than pulling native application integrations |

## Open Risks

- The lightweight builder intentionally accepts host-owned context and tools;
  migrating an application that needs native coding policies requires the
  existing native adapter or an explicit host policy implementation.
- Execution remains cooperative. A backend or tool that ignores cancellation
  can delay completion and shutdown until its future returns.
- Native and host event payloads are different projections of one lifecycle;
  no wire-protocol compatibility is implied between those projections.

## Source Journals

- [Lightweight session ownership](../journal/2026-10-04-lightweight-session-runtime.md)
