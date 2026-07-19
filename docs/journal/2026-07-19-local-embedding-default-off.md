# Local Embedding Default Off

## Summary

Bundled local embedding sidecar startup now defaults to off. RARA still keeps
local memory, retrieval orchestration, LanceDB-backed vector storage, and the
standalone `EmbeddingBackend` boundary, but ordinary startup no longer prepares
or starts the local model-server sidecar unless the user explicitly opts in.

## Background

The previous `auto` default made providers without native embeddings route to
the bundled sidecar during startup. That improved retrieval quality for those
providers, but it also meant ordinary RARA startup could create a Python
environment, download embedding models, or start a sidecar before the user had
asked for that local embedding runtime.

The desired boundary is conservative startup without removing local memory.
External memory providers can integrate with RARA, but they are not replacements
for RARA's local memory path.

## Scope

- Made `LocalEmbeddingPolicy::Off` the default config value.
- Preserved `LocalEmbeddingPolicy::Auto` as an explicit opt-in policy.
- Preserved the existing `RARA_LOCAL_EMBEDDINGS=auto|on|true|1|yes` override as
  an opt-in path.
- Updated embedding route tests so default startup reuses the current backend
  and explicit `auto` keeps the existing local sidecar route matrix.

## Validation

```bash
cargo test -p rara-config local_embeddings -- --nocapture
cargo test embedding_route_ -- --nocapture
cargo check --locked --workspace --all-targets
cargo fmt --check
git diff --check
```

## Follow-Ups

- Add explicit provider override controls if RARA needs user-facing selection
  between provider-native embeddings and the local sidecar beyond `off` and
  `auto`.
