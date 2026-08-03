# Runtime Memory Lifecycle

## Motivation

Session memory follows the runtime session rather than the TUI task completion
path. Local memory remains available when Nowledge Mem is disabled or
unreachable, while hosted capture never blocks the agent loop.

## Ownership

`RuntimeClient` owns one `MemoryLifecycleCoordinator` per session. The
coordinator receives immutable transcript snapshots and is invoked for
incremental capture at turn idle, a pre-compaction flush, and a bounded
shutdown drain. The TUI only renders warning events.

## Sync Contract

Hosted sync uses `rara-{session_id}` as the stable thread ID. Each transcript
message has a stable `rara-msg-{session_id}-{index}` external ID. Requests carry
an idempotency key, `deduplicate=true` on append, `source_app=RARA`, workspace,
space, agent, host-agent provenance, and the lifecycle reason. The source
application marker is the protocol value `RARA`.

Create conflicts fall through to append so an existing thread cannot discard
new messages. The coordinator also filters acknowledged external IDs within
the process lifetime.

## Failure And Shutdown

Transport failures and timeouts emit a structured runtime warning and leave the
agent interaction usable. Shutdown uses a two-second bounded drain; failure is
reported but does not prevent terminal teardown. Local auto-memory follows the
same runtime shutdown boundary.

## Context Refresh

When the builtin plugin is enabled, the runtime appends Nowledge Mem lifecycle
guidance to the default prompt. It requests Context Bundle or Working Memory at
session start when scope or prior decisions matter, and again after
compaction. This keeps prompt context current without making the runtime
dependent on the service.

## Verification

Coordinator tests cover stable IDs, incremental deduplication, retry after
transport failure, and warning emission. `TuiHarness` covers idle,
compaction, duplicate runtime events, shutdown drain, and fail-open behavior
without sleeps or a real network.
