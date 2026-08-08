# Remove Embedded Vector Memory

## Summary

Removed the embedded vector-memory path from the runtime. Local memory now stays
file/text based, while semantic recall is delegated to official Mem.

## Background

The previous memory stack carried three local semantic components:

- LanceDB-backed vector storage;
- an embedding trait on LLM backends;
- a bundled local embedding sidecar.

That design made ordinary memory setup heavier than the product direction
requires. The runtime only needs Claude/Codex-style short-term local memory;
semantic and cross-tool memory should be provided by Mem.

## Scope

- Removed direct LanceDB, Lance index, and Arrow dependencies from Cargo
  manifests.
- Removed `VectorDB`, vector tools, and vector retrieval provider naming.
- Removed `LlmBackend::embed`, embedding backend adapters, and test mocks.
- Replaced the vector handle with a local `MemoryHandle`.
- Kept memory records and session context searchable through deterministic
  local text search.
- Disabled the bundled local embedding runtime and updated TUI/status surfaces
  to report semantic local memory as disabled.
- Kept `local_embeddings` config parsing as deprecated compatibility only.

## Key Decisions

- Local persistence remains authoritative for short-term memory.
- No local fallback semantic index is introduced.
- Serialized session context keeps the old `vector` field as an empty
  compatibility field, but vectors are no longer produced or consumed.
- Historical LanceDB journals remain historical evidence; the current
  contract lives in `docs/features/memory-records.md` and
  `docs/features/session-global-memory.md`.

## Validation

```bash
cargo fmt --all
cargo check
cargo test memory_store -- --nocapture
cargo test session_context -- --nocapture
cargo test context::retriever -- --nocapture
cargo test context::assembler -- --nocapture
rg -n "EmbeddingBackend|EmbeddingInputKind|async fn embed|hashed_embedding|lancedb|LanceDB|VectorDB|vectordb|vector_memory|remember_experience|retrieve_experience" src crates/rara-memory crates/rara-tools Cargo.toml crates/*/Cargo.toml
```

## Follow-Ups

- Regenerate Bazel lock metadata if this branch is intended to be validated
  through Bazel CI.
- Decide whether the deprecated `local_embeddings` config field should remain
  indefinitely for old config compatibility or be removed in a future config
  migration.
