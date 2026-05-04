# LanceDB Memory Index Checkpoint

## Summary

Implemented the first runtime slice of LanceDB-backed memory retrieval.

The old `VectorDB` façade no longer returns empty mock results. It now stores
memory records in LanceDB with:

- raw text;
- session and turn metadata;
- an embedding vector column;
- a LanceDB FTS index over text;
- vector, FTS, and hybrid search helpers.

This checkpoint uses LanceDB vector search over the vector column. Creating and
tuning an ANN index is left to the next performance-oriented slice, after record
volume and embedding dimensions are stable.

## Design Notes

LanceDB is the unified local index backend for this slice, but it is not the
agent-facing memory policy layer.

`MemorySelection` remains the policy boundary for context injection. LanceDB
only produces retrieval candidates and diagnostic scores. This keeps the later
ACP/Wire control-plane work from depending directly on LanceDB APIs.

Search paths do not create tables. Only write paths create tables with the real
embedding dimension. This prevents an empty FTS query from creating a table with
a guessed vector dimension before the first write.

Multiple RARA processes may point at the same workspace LanceDB directory.
Mutation paths therefore use an adjacent advisory lock file (`lancedb.lock`) to
serialize table creation, FTS index creation, and upserts. Read-only vector and
FTS queries stay lock-free unless they need to create the missing FTS index.

The global table shape is not the final session-history design. Raw session
turns should move to per-session append shards, while global memory should be
updated through explicit memory writes or periodic promotion and distillation.

## Runtime Wiring

- Agent turn checkpoints write to the `conversations` LanceDB table.
- `MemoryStore` owns the memory-domain runtime facade over the LanceDB index.
- `remember_experience` writes embedded text through `MemoryStore::insert`.
- `retrieve_experience` runs `MemoryStore::search` and returns
  `relevant_experiences` plus diagnostics.

## Follow-Up

- Move raw session checkpoints into per-session append shards.
- Add periodic promotion from session shards into global `MemoryRecord`s.
- Extend records with provenance, source scope, trust level, labels, and path
  signals in the persisted index before broad automatic writes.

## Memory Product Contract Addendum

The durable memory design is broader than the current LanceDB index slice.
RARA has the retrieval substrate, but not the complete durable memory product
model yet.

The updated `memory-records` spec now records this boundary explicitly:

- LanceDB rows are not the public memory contract.
- `MemoryRecord` owns title, Markdown content, labels, importance, timestamps,
  source, scope, and optional embedding.
- Threads are raw conversation records; memories are distilled durable knowledge.
- `remember_experience` and `retrieve_experience` are compatibility adapters
  over `MemoryStore`.
- Protocol-facing memory APIs must pass through `MemoryStore` and
  `MemorySelection`, not direct LanceDB calls.
