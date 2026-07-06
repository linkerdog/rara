# Benchmark Local Embedding Override

## Summary

Local embedding sidecar startup remains enabled by default through the existing
provider-aware `auto` policy. Terminal-Bench runs disable local embeddings
through the Harbor adapter environment so benchmark startup does not create a
Python environment, download embedding models, or start the model server before
the task begins.

## Scope

- Added `local_embeddings` config with `auto` as the default and `off` as the
  explicit disable policy.
- Added `RARA_LOCAL_EMBEDDINGS=off` environment override for benchmark and
  automation entrypoints.
- Updated the Harbor adapter to pass `RARA_LOCAL_EMBEDDINGS=off` while keeping
  ordinary RARA startup behavior unchanged.

## Validation

```bash
cargo test -p rara-config local_embeddings -- --nocapture
cargo test embedding_route_ -- --nocapture
HARBOR_SITE_PACKAGES=$(find "$(uv tool dir)/harbor/lib" -path '*/site-packages' -type d | head -1)
PYTHONPATH="${HARBOR_SITE_PACKAGES}:tools/harbor:." python -m unittest tools.harbor.test_rara_agent
cargo fmt
git diff --check
```
