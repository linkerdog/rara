# Codex 0.144.3 Dependency Bump

## Summary

RARA now pins its direct Codex git dependencies to the latest stable Rust
release tag, `rust-v0.144.3`. The `0.145.0` tags available at the time of this
change are pre-release alphas and remain out of scope.

The updated crates are:

- `codex-execpolicy`
- `codex-http-client`
- `codex-login`
- `codex-models-manager`

Cargo resolved the Codex dependency graph at commit
`78ad6e6bfd1d3b6a209acd3ef82172a96b25179c`.

## Scope

This is a compatible patch-level update from `0.144.1`. RARA retains the
existing explicit `HttpClientFactory` supplied to `ModelsManager::list_models`,
which was required by the previous `0.144.1` upgrade.

## Validation

```bash
cargo metadata --locked --no-deps --format-version 1
cargo fmt --check
cargo check
```

All commands completed successfully.
