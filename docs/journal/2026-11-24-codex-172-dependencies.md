# Codex 0.172 Dependency Refresh

## Summary

- Upgrade all four direct Codex crates together from `rust-v0.154.0` to the
  latest stable release, `rust-v0.172.0`.
- Match the upstream WebSocket patch revisions and resolve all 44 Codex packages
  to commit `3d86e43b075adeba2193ec5c22ac92f55afd82e6`.
- Adapt the bundled model catalog constructor and OAuth test fixtures without
  changing RARA's authentication or model selection contracts.
- Refresh registry dependencies within existing version requirements. Keep
  unrelated Git revision pins and major-version requirements unchanged.

The refresh is based on `main` at `ebf7bb66`, excluding the unrelated local TUI
branch. Representative registry updates are Tokio 1.53.2 to 1.54.1, RMCP 3.5.0
to 3.11.0, TOML 1.1.3 to 1.2.2, serde_json 1.0.151 to 1.0.154, and the Bedrock
runtime SDK 1.145.0 to 1.157.0.

The existing `aws-lc-sys` header-copy patch is refreshed for 0.47.0's
`copy_writable` call while preserving its symlink handling. Bazel configuration
and unrelated Git pins are unchanged.

## CI Compatibility Correction

The initial CI head `2142f669` failed in `rama-core` before RARA compiled:
`rama_error::OpaqueError` no longer exists at the root of `rama-error` 0.3.0.
The dependency refresh mixed Rama's alpha.4 consumers with stable 0.3.0 utility
crates because their version requirements permit that resolution.

Keep `rama-error`, `rama-macros`, and `rama-utils` locked to
`0.3.0-alpha.4`, matching every other Rama crate and Codex 0.172's upstream
lockfile. Future lockfile refreshes must preserve this cohort until the Codex
dependency graph supports a consistent newer Rama release. No Codex downgrade
or application workaround is needed.

All five failed jobs (build, Clippy, tests, exec CLI, and Bazel) reported this
same compile error. A successful dependency resolution alone did not detect the
upstream API incompatibility; the corrected head still requires CI compilation.

The next CI head `323daff8` passed build, Clippy, and exec CLI. Cargo tests and
Bazel tests exposed a separate Unicode width regression: unicode-width 0.2.3
counts halfwidth sound marks itself, while Ratatui 0.30 and RARA's shared width
policy still add the compensation needed by 0.2.2. Pin unicode-width to exactly
0.2.2 until both width policies can migrate together. Keep the existing terminal
clear-position and sidebar truncation assertions unchanged.

The Bazel PTY editor handoff test also timed out on its first attempt, then
passed both subsequent automatic retries. This remains an observed intermittent
failure, not evidence of a dependency regression; no terminal behavior or test
timeout is changed here.

## Reference Patterns

The Codex 0.172 sources retain the existing authentication entry points, add
optional OAuth provenance and login ownership fields, and require a model catalog
origin. RARA continues to delegate authentication to Codex and identifies its
static catalog as bundled. The local Claude Code OAuth service similarly keeps
token exchange separate from caller-owned account storage; no UI-owned auth
implementation is introduced.

## Validation

- `cargo update` and `cargo metadata --locked --format-version 1` resolve the
  final graph. All declared dependency MSRVs are at most Rust 1.96, the existing
  Bazel toolchain; this is a metadata check, not a compilation result.
- `cargo fmt --all` and `git diff --check` cover formatting and patch hygiene.
- `patch --dry-run -p1` against the downloaded `aws-lc-sys` 0.47.0 source checks
  that the repository patch still applies.
- OAuth and TUI auth fixtures explicitly leave `oauth_provenance` and
  `login_owner_id` unset, retaining coverage for legacy credentials.
- Compilation, Clippy, OAuth/TUI tests, and Bazel validation remain CI gates;
  no local Rust build or test result is claimed.
