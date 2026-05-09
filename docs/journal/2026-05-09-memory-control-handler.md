# Memory Control Handler

## Context

RARA already had durable `MemoryRecord` storage and control-plane request
types, but the memory control handler still emitted placeholder events instead
of touching the memory domain. That meant ACP/Wire-style integrations could not
inspect or mutate memory without inventing an adapter-specific path.

## Change

- `MemoryControlHandler` now owns an `Arc<MemoryStore>` and routes memory
  control requests through the memory-domain facade.
- `AddRecord` persists a protocol-written `MemoryRecord` with validated labels,
  scope, optional title, importance, pin state, session id, and thread id.
- `UpdateRecord`, `DeleteRecord`, `ListLabels`, and `QueryRecords` call the
  corresponding `MemoryStore` APIs.
- Runtime memory events now include structured label counts and stable record
  event views for query responses.
- Control-plane dispatch now returns memory handler errors instead of
  swallowing invalid requests.

## Boundary

This keeps protocol adapters away from LanceDB and the on-disk record format.
ACP, Wire, and future appserver clients should send typed
`MemoryControlRequest` values and consume `MemoryEvent` responses. Prompt
injection still goes through retrieval orchestration and `MemorySelection`; a
protocol memory write does not directly edit the model prompt.

## Validation

- `cargo test memory_control_handler --locked`
- `cargo test memory_update_delete_and_label_requests_use_structured_wire_shape --locked`

Both tests passed locally. The broader crate still has pre-existing warnings
outside this slice.
