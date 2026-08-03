# Runtime Memory Lifecycle Checkpoint

## What Changed

RARA now creates a session-scoped memory lifecycle coordinator with the runtime
client. It captures unsent transcript messages at turn idle, flushes before
compaction, and drains with a bounded timeout during shutdown. The hosted
transport uses stable thread and external message IDs, append idempotency, and
structured provenance metadata with `source_app=RARA`.

The TUI controller consumes an injected runtime port and ignores duplicate or
late runtime events using session and sequence identity. Redraw is requested
only when a projection is accepted. Local auto-memory completion and shutdown
hooks now run from the runtime command processor, preserving local storage
while keeping lifecycle ownership out of TUI task completion.

The built-in Nowledge Mem instructions request context refresh after
compaction, with fail-open behavior when the service is unavailable.

## Trade-offs

The existing local auto-memory implementation still owns its extraction
algorithm and persistence format. This change moves its lifecycle trigger to
the runtime without changing that local backend. Hosted capture remains
best-effort and process-local deduplication is supplemented by server-side
external IDs, append deduplication, and idempotency keys.

## Verification

Focused memory and harness tests cover idle capture, compaction deltas, retry
after failure, duplicate runtime events, warnings, and shutdown drain. Cargo
formatting and workspace checks are required before submission.
