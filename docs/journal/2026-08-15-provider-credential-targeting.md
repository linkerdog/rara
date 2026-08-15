# 2026-08-15 Provider Credential Targeting

## Summary

The TUI API-key flow now carries an explicit provider target from `/connect`
through prompt rendering, credential persistence, and model-catalog refresh.
Choosing Kimi while Codex is active therefore shows Kimi copy and stores the
key in the Kimi profile without overwriting the active Codex credential.

## Background

The API-key overlay previously inferred its provider from mutable TUI and
configuration state. `/connect` could select Kimi while the active provider
remained Codex, causing the modal to request a Codex key and the save handler
to write the submitted value into Codex state.

OpenCode avoids this ambiguity by carrying `providerID` or `integrationID` as
an explicit connection-flow input. Its TUI `ApiMethodProps` sends that exact
identifier to `auth.set`, and its app connection dialog sends the selected
integration identifier with the API key. Codex and Claude Code have
single-provider authentication flows, but likewise encode the selected auth
method in typed flow state instead of reconstructing it from model state.

## Scope

- Added a typed API-key target to the setup overlay.
- Made provider-specific modal copy exhaustive for Codex, DeepSeek, Kimi,
  Gemini, and custom OpenAI-compatible profiles.
- Added provider-scoped credential updates that preserve the active provider.
- Routed model-catalog requests through a target-provider configuration copy so
  API keys and base URLs cannot leak across providers.
- Added config, render, event-flow, and runtime-routing regression coverage.

## Key Decisions

- Provider identity is captured when the connection step opens and remains the
  authority for that step.
- Credential storage remains in the existing `provider_states` and
  `openai_profiles` compatibility model; this fix does not move provider state
  ownership into the TUI.
- Model-catalog routing switches only a cloned configuration to the target
  provider. The user's active configuration remains unchanged.

## Validation

Validation commands:

```bash
cargo fmt --all
cargo test -p rara-config setting_inactive_provider_api_key_preserves_active_provider_credentials
cargo test kimi_api_key_editor_uses_explicit_target_when_codex_is_active
cargo test kimi_connection_saves_to_kimi_without_overwriting_active_codex_key
cargo test model_catalog_connection_uses_target_provider_credentials
cargo test api_key
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

## Follow-Ups

No new follow-up is introduced. The existing provider-connection follow-up to
move availability and authentication projection into the session-scoped
runtime control plane remains applicable.
