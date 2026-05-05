# Memory Control Scaffolding Checkpoint

## Summary

RARA now has memory-domain operations needed by future ACP/Wire memory control:

- `MemoryStore::update` applies a typed patch to durable `MemoryRecord` data.
- `MemoryStore::delete` removes a durable record from the domain store.
- `MemoryStore::list_labels` returns label counts, optionally scoped.
- `RuntimeControlRequest::Memory` now includes update, delete, and list-label
  request shapes.

## Design Boundary

Protocol adapters should call the `MemoryStore` boundary. They should not edit
`records.json` directly and should not call LanceDB APIs directly.

The durable record file remains the source of truth. LanceDB is still the local
retrieval index. When a record is deleted, stale indexed rows are filtered at
rehydration time so deleted records do not reappear in search results. Physical
index cleanup can be added later without changing the public memory contract.

## Validation

Focused coverage was added for:

- record update with content re-indexing;
- delete hiding stale LanceDB hits;
- label counts with scope filtering;
- structured runtime-control request shapes for update, delete, and list-label
  operations.
