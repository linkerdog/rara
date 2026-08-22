# Remove Deprecated `local_embeddings` Config Field

## What

Removed the last remaining local-embedding config surface, which had been kept
as deprecated compatibility-only since
`docs/journal/2026-08-08-remove-embedded-vector-memory.md`:

- `crates/config/src/model.rs` — deleted the `LocalEmbeddingPolicy` enum
  (`Off` / `Auto` / `Provider` / `Local`) and its `is_default` helper.
- `crates/config/src/model/rara_config.rs` — deleted the `local_embeddings`
  field, its serde skip rule, and the `RARA_LOCAL_EMBEDDINGS` environment
  parsing in `apply_provider_environment_defaults_from`.
- `crates/config/src/lib.rs` — dropped the `LocalEmbeddingPolicy` re-export.
- `crates/config/src/model_test.rs` — deleted the four `local_embeddings`
  config tests.
- `tools/harbor/rara_agent.py` — stopped injecting `RARA_LOCAL_EMBEDDINGS=off`
  for benchmark runs; `tools/harbor/test_rara_agent.py` test renamed and the
  assertion removed.

## Why

The bundled local embedding sidecar was already removed and local semantic
memory delegated to Nowledge Mem, so the field had zero runtime consumers and
only existed to let stale configs deserialize. Keeping dead config invites
maintenance drift and false signals in provider/status surfaces.

## Trade-offs

- Old config files that still set `local_embeddings` will now be silently
  ignored by serde rather than parsed. This is acceptable because the field no
  longer had any behavior attached and its prior value was never acted on.

## Remains

- Nothing for this field. The unrelated Nowledge Mem read-side runtime
  pre-fetch is still tracked separately in `docs/todo.md` under Memory
  Lifecycle.
