# Plugin Explicit Directories

## Summary

RARA now accepts repeated `--plugin-dir <path>` global CLI flags for TUI and
`resume` sessions. These directories are passed into the TUI runtime and used as
the final Claude plugin discovery source tier during hook registration.

## Background

The previous plugin runtime source work added user and project plugin discovery
plus a middleware parameter for explicit plugin directories, but there was no
user-facing CLI path that filled that parameter. This left manual plugin
directory testing dependent on code-level call sites.

## Scope

- Added a global `--plugin-dir DIR` CLI flag.
- Normalized parsed directories to absolute paths during CLI startup.
- Preserved the normalized directories through TUI and `resume` startup.
- Stored explicit plugin directories on `TuiApp` so runtime rebuild hook
  registration can pass them into `register_plugin_hooks`.
- Triggered the TUI runtime rebuild path when explicit plugin directories are
  supplied so plugin hook registration runs even when local embedding startup is
  disabled.
- Kept headless, ACP, and Wire runtime startup out of scope because those
  surfaces still need plugin hook execution ownership work.

## Validation

```bash
cargo test app_cli::tests::clap_parses_explicit_plugin_dirs_as_global_args -- --nocapture
cargo test app_cli::tests::clap_parses_explicit_plugin_dirs_after_tui_command -- --nocapture
cargo test app_cli::tests::normalize_plugin_dirs_returns_absolute_paths -- --nocapture
cargo test tui::state::tests::new_starts_without_explicit_plugin_dirs -- --nocapture
cargo test plugin_middleware::tests::plugin_discovery_sources_order_user_project_then_cli -- --nocapture
cargo check --locked --workspace --all-targets
cargo fmt --check
git diff --check
```

## Follow-Ups

- Add config persistence for explicit plugin directories.
- Extend plugin runtime startup beyond TUI once headless, ACP, and Wire define
  their hook runtime execution boundary.
