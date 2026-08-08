# Codex Rust Dependency Upgrade

## Change

RARA's OpenAI Codex Rust dependencies were upgraded from `rust-v0.145.0` to
the latest stable release, `rust-v0.146.0`. The lockfile now resolves the
complete Codex dependency graph at commit `e363b08c`.

## Compatibility

Codex login now requires an explicit `AuthRouteConfig`. RARA supplies the
default Codex `HttpClientFactory` to both model-catalog authentication and the
browser login server. No provider behavior or credential storage policy was
changed.

## Verification

- `cargo fmt --all`
- `cargo check --all-targets`
- `cargo test --bin rara oauth --no-fail-fast`
- `cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings`
- `git diff --check`

## Follow-up

The `0.147.0-alpha.*` tags are prereleases and are intentionally not used for
the stable dependency path.
