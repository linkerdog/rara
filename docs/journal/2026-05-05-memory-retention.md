# Memory Retention Checkpoint

## Summary

RARA now persists explicit retention metadata on durable memory records:

- `MemoryRecord.pinned` is part of the domain record and defaults to `false`
  when older record files are loaded.
- `NewMemoryRecord.pinned` lets memory creation callers mark records as pinned
  without bypassing `MemoryStore`.
- `MemoryStore::set_pinned` updates the durable record file and leaves the
  LanceDB search row unchanged because pinning does not alter searchable text or
  embeddings.
- `MemoryRecord::is_protected_from_automatic_cleanup` centralizes the retention
  rule: pinned records, user-created records, and records with importance `0.8`
  or higher are protected from future automatic cleanup paths.

## Design Boundary

This slice does not add automatic cleanup. It intentionally lands the retention
contract first so later cleanup, promotion, and protocol memory mutation code
must call the same domain guard instead of re-implementing policy at the edge.

The shape follows the existing memory/context boundary: LanceDB remains the
retrieval index, while durable memory policy lives in `MemoryStore` and
`MemoryRecord`.

## Validation

Focused coverage was added for:

- retention protection across pinned, user-created, and high-importance records;
- persisted pin updates through `MemoryStore::set_pinned`;
- existing record persistence/search tests now preserving pinned metadata.
