# Benchmark Local Embedding Override

## Summary

This checkpoint introduced the initial local embedding sidecar policy before
the later default-off change. At the time, local embedding sidecar startup
remained enabled by default through the provider-aware `auto` policy, while
Terminal-Bench runs disabled local embeddings through the Harbor adapter
environment so benchmark startup did not create a Python environment, download
embedding models, or start the model server before the task began.

See `docs/journal/2026-07-19-local-embedding-default-off.md` for the follow-up
that made bundled local embedding startup default-off while preserving explicit
opt-in.

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
