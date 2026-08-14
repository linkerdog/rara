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
- Removed stale legacy provider commands from the hard-coded `/help` guidance.

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
cargo test help_text_exposes_only_current_provider_entrypoints -- --nocapture
cargo clippy -- -D warnings
```

The CI regression was caused by stale hard-coded `/help` text, rather than the
command registry. The focused test now verifies both the absence of retired
provider commands and the presence of `/connect` and `/model`.

`cargo clippy --all-targets --all-features -- -D warnings` is not applicable
on Linux because the all-features set enables the macOS-only `objc2` crate.

## Follow-Ups

- Publish provider availability and authentication methods from the
  session-scoped runtime instead of calculating them in `TuiApp`.
- Replace remaining legacy `ListPickerKind::Model` and `UnifiedModel` paths
  with the sole `/model` picker after their callers are migrated.
