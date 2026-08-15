# 2026-08-15 Provider Credential Targeting

## Summary

The TUI API-key flow now carries an explicit provider target from `/connect`
through prompt rendering, credential persistence, and model-catalog refresh.
Choosing a provider while Codex is active therefore shows provider-specific
copy and stores the key in the selected profile without overwriting the active
Codex credential.

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
- Split Kimi For Coding from Moonshot AI so each provider owns one endpoint,
  environment variable, and persisted endpoint profile.

## Key Decisions

- Provider identity is captured when the connection step opens and remains the
  authority for that step.
- Credential storage remains in the existing `provider_states` and
  `openai_profiles` compatibility model; this fix does not move provider state
  ownership into the TUI.
- Model-catalog routing switches only a cloned configuration to the target
  provider. The user's active configuration remains unchanged.
- The existing serialized `kimi` endpoint kind remains the Moonshot Open
  Platform profile for compatibility. The new `kimi_coding` endpoint kind uses
  `https://api.kimi.com/coding/v1` and `KIMI_API_KEY`.
- `MOONSHOT_API_KEY` and `KIMI_API_KEY` are not fallback aliases. Kimi documents
  the two credential domains as non-interchangeable, and OpenCode models them
  as separate providers.

## Follow-Up Correction

The first credential-targeting implementation still presented the historical
Moonshot profile as `Kimi`. A valid Kimi Code subscription key was therefore
sent to `https://api.moonshot.ai/v1/chat/completions` and rejected as invalid
authentication. The follow-up keeps that persisted profile as Moonshot AI for
backward compatibility and adds an independent Kimi For Coding profile with a
stable `kimi-for-coding` model alias.

The first follow-up stored the new credential without activating its profile.
When Moonshot was already active, the next query therefore still used the
Moonshot URL. Saving a Kimi For Coding key now activates the coding profile and
requests a session backend rebuild while retaining the previous provider's
credential in provider-scoped state.

## Validation

Validation commands:

```bash
cargo fmt --all
cargo test -p rara-config setting_inactive_kimi_coding_key_uses_the_coding_profile
cargo test -p rara-config kimi_coding_profile_uses_the_dedicated_coding_endpoint
cargo test -p rara-config moonshot_profile_does_not_use_kimi_code_environment_key
cargo test kimi_coding_api_key_editor_names_the_dedicated_credential_domain
cargo test kimi_coding_connection_uses_the_dedicated_profile_and_endpoint
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
