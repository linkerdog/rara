# Codex 0.131 Dependency Bump

## Summary

RARA now pins its direct Codex git dependencies to the latest stable Codex Rust
release tag, `rust-v0.131.0`.

The updated crates are:

- `codex-execpolicy`
- `codex-login`
- `codex-models-manager`

Cargo resolved the full Codex dependency set from `0.130.0` to `0.131.0`. The
tag resolves to commit `05eb8678451435cbc8d79c6d8254276289f2bdf1`.

The direct Codex dependency definitions live in `[workspace.dependencies]`, and
the main `rara` package references them with `workspace = true` so future Codex
updates only need to change one version target.

## Scope

This is a dependency maintenance update only. No RARA runtime behavior, TUI
contracts, configuration shape, or public protocol surface changed in this
checkpoint.

The lockfile refresh intentionally avoids unrelated transitive downgrades. The
remaining non-Codex lockfile additions are required by the resolved Codex
`0.131.0` dependency graph.

## Validation

```bash
gh release list --repo openai/codex --limit 10
git ls-remote https://github.com/openai/codex.git refs/tags/rust-v0.131.0 refs/tags/rust-v0.131.0^{}
cargo update -p codex-login -p codex-models-manager -p codex-execpolicy
cargo fmt --check
cargo check
cargo clippy
cargo test
```

`cargo clippy` completed successfully with the workspace's existing warnings.
`cargo test` passed with 1065 tests.

## Follow-Ups

- Watch CI for platform-specific dependency resolution issues.
- Revisit Codex pre-release `0.132.0-alpha.*` only after it becomes a stable
  release.
