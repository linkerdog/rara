# Codex Rust 0.147 Upgrade

## Change

RARA's OpenAI Codex Rust dependencies were upgraded from `rust-v0.146.0` to
`rust-v0.147.0`. The lockfile now resolves the complete Codex dependency graph
at commit `be6e8eac`.

## Compatibility

The upgrade keeps the existing RARA integration points unchanged. The direct
workspace dependencies remain:

- `codex-execpolicy`
- `codex-http-client`
- `codex-login`
- `codex-models-manager`

The Harbor adapter workflow is intentionally unchanged. The failing CI path was
treated as dependency graph drift between Codex versions, while Harbor remains a
regression validation surface.

## Verification

- `cargo check -p rara --locked`
- `cargo fmt --all -- --check`
- `cargo tree --locked | rg "codex-[a-z0-9_-]+ v0\\.(146|147)\\.0"`
- `cargo tree -i codex-login --locked`
- `bazel test //crates/config:rara_config_tests`
- `git diff --check`

## Follow-up

- Run the existing Harbor adapter workflow in CI as the regression gate.
