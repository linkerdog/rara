# Plugin Directory Config

## Summary

RARA now persists explicit Claude plugin directories in `plugin_dirs` config and
uses them for TUI and `resume` plugin hook registration.

## Background

The previous plugin explicit-directory slice added repeated `--plugin-dir DIR`
CLI flags, but those directories only lived for the current process. Users who
always load the same local plugin directory still needed to pass the CLI flag on
every TUI or `resume` startup.

## Scope

- Added `RaraConfig.plugin_dirs: Vec<PathBuf>` with an empty default that is
  omitted from serialized config.
- Loaded configured plugin directories from `plugin_dirs`.
- Merged configured plugin directories before CLI plugin directories so CLI
  inputs remain the most explicit override within the explicit source tier.
- Reused the existing absolute-path normalization and TUI startup rebuild path.

## Validation

```bash
cargo test -p rara-config plugin_dirs -- --nocapture
cargo test app_cli::tests::effective_plugin_dirs_put_cli_dirs_after_config_dirs -- --nocapture
cargo test app_cli::tests::normalize_plugin_dirs_returns_absolute_paths -- --nocapture
cargo check --locked --workspace --all-targets
cargo fmt --check
git diff --check
```

## Follow-Ups

- Extend plugin source composition beyond TUI once headless, ACP, and Wire have
  a clear hook runtime execution boundary.
- Implement lifecycle parity for `SessionEnd`, matcher evaluation, blocking
  hook results, and hook output observability.
