# Codex 0.144.1 Dependency Bump

## Summary

RARA now pins its direct Codex git dependencies to the latest stable Rust
release tag, `rust-v0.144.1`.

The updated crates are:

- `codex-execpolicy`
- `codex-http-client`
- `codex-login`
- `codex-models-manager`

Cargo resolved the Codex dependency graph from `0.143.0` to `0.144.1` at
commit `44918ea10c0f99151c6710411b4322c2f5c96bea`.

## Scope

Codex `0.144.1` requires callers of `ModelsManager::list_models` to provide an
HTTP client factory. RARA's model-catalog adapter now passes the upstream
`ReqwestDefault` proxy policy explicitly, preserving its previous default HTTP
behavior.

## Validation

```bash
cargo metadata --locked --no-deps --format-version 1
cargo fmt --check
cargo check
cargo test oauth::tests
cargo test codex_provider_family
```

All commands completed successfully. The focused tests passed with 10 OAuth
tests and 3 Codex provider-routing tests.
