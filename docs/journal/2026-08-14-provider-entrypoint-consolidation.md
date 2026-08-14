# 2026-08-14 Provider Entrypoint Consolidation

## Summary

The TUI provider surface now treats `/connect` and `/model` as its only
first-class provider entry points. Legacy `/login`, `/auth`, `/logout`,
`/base-url`, and `/models` commands are no longer parsed or shown in the TUI
command palette.

## Background

Provider setup had split credentials, endpoint configuration, and model choice
over several commands and parallel picker paths. OpenCode instead presents a
provider connection flow and a provider-scoped model picker backed by a common
provider catalog. RARA adopts that command boundary while preserving existing
configuration storage during the transition.

## Scope

- Removed redundant TUI provider commands and their local command kinds.
- Kept base URL, profile, OAuth, API-key, and logout actions as internal
  `/connect` flow steps.
- Made configured providers re-enter their configuration flow instead of
  stopping at an "already connected" notice.
- Limited `/model` to models from providers with a configured connection and
  added an empty-state recovery message pointing to `/connect`.
- Added focused command and TUI-state tests.

## Key Decisions

- CLI compatibility commands remain outside the TUI command-surface contract.
- Existing configuration fields remain the temporary availability adapter.
- Credential presence is only a temporary availability approximation. A
  session-scoped runtime projection must later distinguish configured,
  verifying, available, and failed states.

## Validation

Completed:

```bash
cargo fmt --all
git diff --check
```

The focused Rust tests and `cargo check` remain pending because another build
process holds Cargo's build-directory lock, including when an isolated target
directory is requested. Re-run after that lock is released:

```bash
CARGO_TARGET_DIR=/tmp/rara-provider-entrypoints-target cargo test provider_management_commands_are_not_tui_entry_points -- --nocapture
CARGO_TARGET_DIR=/tmp/rara-provider-entrypoints-target cargo test available_unified_model_presets_exclude_unconfigured_remote_providers -- --nocapture
CARGO_TARGET_DIR=/tmp/rara-provider-entrypoints-target cargo check
```

## Follow-Ups

- Publish provider availability and authentication methods from the
  session-scoped runtime instead of calculating them in `TuiApp`.
- Replace remaining legacy `ListPickerKind::Model` and `UnifiedModel` paths
  with the sole `/model` picker after their callers are migrated.
