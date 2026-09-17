# App Server Stdio Protocol

## Problem

External process supervisors need a stable structured transport over the canonical
runtime session. Shared request enums alone do not define startup readiness,
request acceptance, replay lifetime, or graceful process shutdown. Parsing TUI,
print, Wire, or ACP output cannot establish this contract.

## Scope

- Version1 UTF-8 JSON Lines over one child process's stdin and stdout.
- A server-first handshake with explicit runtime identity and capabilities.
- Correlated runtime-control requests, replay requests, and semantic shutdown.
- Bounded frames and safe protocol errors.
- Canonical RuntimeSession ownership, event ordering, and cleanup.

The codec is the first implementation checkpoint. The CLI, session dispatcher,
receipt registry, and child-process smoke remain required before this protocol is
advertised as available. Declaring a request or acknowledgement type does not
establish that the runtime implements that capability.

## Non-Goals

- Network listeners, remote authentication, or shared multi-tenant hosting.
- Compatibility with another product's JSON-RPC or stream-json schema.
- Reconstructing control events from terminal text.
- Durable approval callbacks or automatic retries after process replacement.
- A second agent owner, scheduler, event sequence allocator, or transcript store.

## Architecture

The existing app-server crate owns the wire vocabulary and bounded serialization.
The application adapter owns process transport, per-runtime request receipts and
subscriptions. RuntimeSession owns agent mutation, turn serialization, native
cancellation, source registries, and shutdown. RuntimeHost owns session handles.
The existing event bus supplies event identity, sequence values and replay gaps.

The adapter must not call the older control-plane dispatcher with a raw mutable
Agent or allocate replacement event sequences. Unsupported session operations
return an explicit rejection and are absent from negotiated request methods.

## Contracts

### Startup

The command is `rara app-server --protocol-version 1 --transport stdio-jsonl`.
Its first stdout line is a `handshake` frame. All diagnostics go to stderr. No
runtime-control request may be sent until the supervisor validates this frame.

Every frame uses the `type` and `payload` envelope. The handshake payload contains:

- `protocol_version`: exactly1;
- `runtime_version`: a nonempty runtime build/version label;
- `runtime_id`: a fresh opaque process incarnation identity;
- `transport`: exactly `stdio-jsonl`;
- `request_families` and `request_methods`: the implemented families and exact
  semantic operation identifiers;
- `event_families`: the event family identifiers the server can emit;
- `capabilities`: explicit shutdown, replay, receipt, and approval lifetimes;
- `provider` and `model`: optional safe labels, never credential/config objects.

Methods are the capability unit. A family with one implemented operation must not
imply support for all operations in its shared request enum. Request families can
be derived from each method's prefix and must match the advertised family list.
Consumers require the methods they use,
including `server.shutdown`, rather than assuming a fixed enum is operational.
Unknown additional method/family identifiers may be ignored, but required methods
must be present. Incompatible protocol or transport rejects startup.

Runtime IDs, request IDs, session IDs, method names, and family identifiers contain
1-128 ASCII bytes from letters, digits, `.`, `_`, `:`, `/`, and `-`. Runtime
version/provider/model labels contain1-256 UTF-8 bytes and no control characters.
Capability lists contain at most64 distinct identifiers.

### Client Frames

`control` carries `runtime_id` and an existing `RuntimeControlEnvelope` as
`envelope`. Its `request_id` identifies the attempted semantic operation; the
target session is the envelope provenance's session ID when required. Transport
authority and source trust are derived from the server-owned process/session
context, never accepted merely because the envelope asserts them.

`control.expected_turn_id` names the originating turn for user/plan/shell or
generic approval answers, and the active or waiting turn for cancel/interrupt.
These operations require both this field and the envelope session target. Other
operations must not provide a turn target. The codec rejects missing, misplaced or invalid targets
before dispatch; the session actor checks the target against current state before
acceptance. A late reply cannot consume a newer wait. Receipt comparison includes
the expected turn as part of the original request, not just the envelope body.

`replay` carries `runtime_id`, `request_id`, `session_id`, and `after_sequence`.
It requests the existing session stream after the given exclusive cursor. The
adapter must preserve original event IDs and sequence values.

`shutdown` carries `runtime_id` and `request_id`. It requests that the process stop
accepting work and drain all owned sessions. It is a semantic command, not a
signal or inferred EOF success.

All client frames are fenced to the current runtime ID. A request carrying an old
incarnation ID is rejected before dispatch. Unknown top-level frame fields are
rejected. Runtime-control request semantics remain owned by the shared request
types and their session dispatcher.

### Request Acknowledgements

An `ack` frame carries `runtime_id`, `request_id`, and a tagged `result`:

- `accepted`: the operation was admitted or applied. Optional session/turn IDs
  and the observed sequence identify its effect; they do not assert completion.
- `queued`: the operation was admitted for later ordered execution. This state
  is emitted only if the negotiated operation actually supports queuing.
- `rejected`: no new operation was admitted; `code` is a stable enum and
  `message` is a safe bounded explanation.

No `unknown` server acknowledgement exists. A supervisor derives uncertainty
when transport closes before the correlated acknowledgement. It must not turn
that uncertainty into an unsafe fresh submission.

Within an advertised runtime receipt lifetime, repeating an identical request ID
and semantic content returns the retained acknowledgement without executing the
operation again. Reusing the ID for different content returns `request_conflict`.
Bounded receipt capacity must reject new work before forgetting accepted request
identity; eviction is not permission to repeat a side effect. A replacement
runtime has a new ID and cannot establish that an old ambiguous operation did not
run. Cross-process receipt persistence is not advertised by this version.

### Events And Replay

An `event` frame carries `runtime_id` and the canonical RuntimeControlEvent as
`event`. Event identity is scoped by runtime/session, independent of request ID.
The event remains typed by the runtime; the shared codec is generic over that
event type so the protocol crate does not depend on the application runtime.

Replay is explicitly bounded in memory. `replay_gap` carries the correlated
request ID, session ID, requested cursor, oldest available sequence and latest
sequence. Consumers retain this gap in their history instead of displaying an
incomplete stream as complete. A snapshot is not a substitute for missing events.
The advertised lifetime is `runtime`, not durable storage or process restart.

### Shutdown And Failure

An accepted shutdown request starts drain. `shutdown_complete`, correlated to the
same request ID, is emitted only after owned session/child cleanup succeeds. An
acknowledgement alone is not cleanup evidence. The supervisor may enforce its
own timeout and force-stop policy when shutdown cannot finish.

EOF, write failure, invalid framing, and oversized output cause transport failure
and explicit runtime cleanup. They must not be reported as successful semantic
shutdown. Live approval callbacks cannot survive this boundary unless a future
capability explicitly implements and proves persistence.

### Resource And Error Bounds

Each JSON payload is at most1,048,576 bytes excluding its LF delimiter. A frame
must be a single UTF-8 JSON object. Encoders append exactly one LF. Serialization
must enforce the bound while writing, not after allocating an arbitrarily large
event. The transport reader must independently enforce its buffer limit before
decoding. CRLF compatibility, partial EOF handling and connection queue capacities
must be proved when the process transport is added.

Protocol codec failures expose fixed error categories, without reflecting raw
input, JSON values, paths, provider errors, or secrets. Runtime rejection messages
must likewise be selected/redacted by the dispatcher; byte validation alone is
not secret redaction.

## Validation Matrix

| Boundary | Required evidence |
| --- | --- |
| Codec | Golden handshake/control/ack/replay/shutdown shapes and bounded serialization |
| Validation | Empty/invalid IDs, duplicate/oversized capability lists, wrong protocol/transport, invalid UTF-8/JSON and unknown frame fields |
| Identity | Stale runtime rejection, request conflict, duplicate without a second effect |
| Session | Busy/cancel/interrupt and source control through the canonical session owner |
| Replay | Original event identity, cursor catch-up, bounded gap and no cross-runtime continuation claim |
| Process | Real isolated child emits handshake first, accepts one prompt, emits typed events and drains on shutdown |
| Failure | Truncated/oversized input, stdout write loss, stderr isolation and retained ambiguity |

## Operational Notes

Start with local subprocess placement and explicit capabilities. Provider config
and credentials remain runtime-owned. A supervisor must retain its own durable
attempt/event journal and cleanup authority independently of this transport.

## Open Risks

- The transport and canonical command seam must be implemented before the CLI
  capability is available; codec fixtures alone are not a production adapter.
- The existing session API does not yet expose every shared control request.
- In-memory receipts and replay do not make external side effects crash-safe.

## Source Journals

- [Stdio protocol foundation](../journal/2026-09-18-app-server-stdio.md)
