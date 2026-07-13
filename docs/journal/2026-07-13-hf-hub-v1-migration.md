# hf-hub 1.0 Migration

## Summary

RARA now uses `hf-hub` 1.0 with its blocking typed repository client for local Candle models and managed embedding-model snapshots.

## Background

The 1.0 release removes the legacy synchronous `api`, `Cache`, and `Repo` interfaces. The Dependabot update changed only the lockfile, leaving the manifest and all local-model download paths incompatible with the new release.

## Key Decisions

- Enable the upstream `blocking` feature rather than creating a RARA-owned Tokio runtime.
- Preserve `RARA_MODEL_CACHE`, `HF_ENDPOINT`, token resolution, three retry attempts, revision selection, and the `models--owner--name` cache layout.
- Map the new thread-safe progress events onto the existing TUI download messages.

## Validation

- `cargo fmt --all`
- `cargo check --locked`
- `cargo test --locked local_model_server::tests`

## Follow-Ups

- CI validates the complete workspace and platform-specific download behavior.
