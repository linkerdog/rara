# Local Embedding Opt-In Default

## Summary

Local embedding sidecar startup is now opt-in. The default config keeps local
embeddings off and uses the current `LlmBackend` embedding path, avoiding
unexpected Python setup, model downloads, or model-server startup during normal
CLI, TUI, ACP, and benchmark runs.

## Scope

- Added `local_embeddings` config with `off` as the default and `auto` as the
  explicit sidecar policy.
- Changed runtime embedding routing so `off` always reuses the current backend.
- Preserved the existing provider-aware local sidecar routing when
  `local_embeddings` is set to `auto`.

## Validation

```bash
cargo test -p rara-config local_embeddings -- --nocapture
cargo test embedding_route_ -- --nocapture
cargo fmt
git diff --check
```
