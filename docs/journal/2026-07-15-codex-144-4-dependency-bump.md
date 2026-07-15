# Codex 0.144.4 Dependency Bump

## Summary

RARA now pins its direct Codex git dependencies to the latest stable Rust
release tag, `rust-v0.144.4`. The available `0.145.0` tags remain pre-release
alphas and are intentionally excluded.

The updated crates are:

- `codex-execpolicy`
- `codex-http-client`
- `codex-login`
- `codex-models-manager`

## Scope

This is a patch-level update from `0.144.3`. The dependency lockfile records
the complete resolved Codex graph at the selected release tag.

## Validation

- `cargo metadata --locked --no-deps --format-version 1`
- `cargo fmt --check`
- `cargo check`
