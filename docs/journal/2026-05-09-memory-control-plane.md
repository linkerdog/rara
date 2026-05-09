# Memory Control Plane Execution

## Context

RARA already had structured memory control request types for ACP/Wire-style
adapters, but the protocol memory handler only emitted placeholder events. That
meant protocol clients could describe memory mutations, but they could not
actually inspect or mutate durable memory through the runtime boundary.

## Change

`MemoryControlHandler` now has a `MemoryStore`-backed execution path:

- `AddRecord` writes a protocol-origin `MemoryRecord` through `MemoryStore`;
- `UpdateRecord` maps protocol patches to `MemoryRecordPatch`;
- `DeleteRecord` removes the durable record through `MemoryStore`;
- `ListLabels` returns structured label counts;
- `QueryMetadata` returns record count plus label counts.

All successful operations publish structured `MemoryEvent` values on the
runtime event bus. This keeps ACP/Wire integrations behind the same domain
facade as local tools and avoids direct prompt-text mutation or LanceDB access.

## Boundaries

- Retrieved memory placement remains governed by `MemorySelection` and
  `ContextAssembler`.
- Protocol memory writes do not inject text into the stable prompt prefix.
- Transport-specific ACP/Wire command surfaces still need to call the shared
  control-plane dispatcher.

## Validation

Focused tests cover:

- protocol add writes a durable `MemoryRecord` with protocol provenance and
  emits the stored id;
- protocol update/list/delete use the same `MemoryStore` record truth and emit
  structured events.
