# Codex 0.144.6 Dependency Bump

## Summary

RARA now pins its direct Codex git dependencies to the latest stable Rust
release tag, `rust-v0.144.6`. The available `0.145.0` tags remain pre-release
alphas and are intentionally excluded.

The updated crates are:

- `codex-execpolicy`
- `codex-http-client`
- `codex-login`
- `codex-models-manager`

Cargo resolved the Codex dependency graph at commit
`5d1fbf26c43abc65a203928b2e31561cb039e06d`.

## Scope

This is a compatible patch-level update from `0.144.4`. No RARA source
adaptation was required.

## Validation

```bash
gh release list --repo openai/codex --limit 20
git ls-remote https://github.com/openai/codex.git refs/tags/rust-v0.144.6 refs/tags/rust-v0.144.6^{}
cargo metadata --locked --no-deps --format-version 1
cargo fmt --check
cargo check
```

All commands completed successfully.
