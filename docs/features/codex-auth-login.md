# Codex Auth Login

## Summary

RARA currently supports a lightweight provider-specific OAuth flow for the
Codex preset in `src/oauth.rs`. If the goal is to support Codex's own login
mode more faithfully, the primary reference is Codex's `codex login` flow, not
the MCP/appserver OAuth flow.

Codex's first-party login flow is implemented around `codex_login` and supports:

- browser login via a local callback server;
- device-code login for headless/remote environments;
- API key login;
- logout and persistent credential storage.

RARA should align with that behavior first. MCP/appserver OAuth remains relevant
later, but it is a separate second-layer concern.

## Goals

- Support Codex-style first-party login modes:
  - browser login
  - device-code login
  - API key login
- Keep provider auth/runtime state compatible with Codex-style credential
  persistence and login choices.
- Prefer reusing Codex crates for login execution and credential handling
  instead of maintaining a parallel OAuth implementation in RARA.
- Stop hardcoding the Codex model list inside RARA once login succeeds; reuse
  Codex's own model catalog and reasoning-level metadata instead.
- Make the Codex-auth path usable from:
  - CLI
  - TUI login flows
  - future appserver/ACP integration that depends on Codex-authenticated
    provider access

## Non-Goals

- Implementing MCP server OAuth login in the same change.
- Implementing the full ACP runtime in the same change.
- Recreating the entire Codex CLI/config/runtime stack inside RARA.

## Current State

RARA now has a first implementation pass of Codex-style first-party auth:

- `rara login`
  - browser login by default;
  - `--device-auth` for headless/remote flows;
  - `--with-api-key` for stdin-fed API keys;
- `rara logout`;
- a Codex auth picker in the TUI with:
  - browser login
  - device code
  - API key
  - logout

This implementation now routes the core login actions through `codex_login`
instead of keeping a separate local OAuth protocol mirror in RARA.

Codex has a richer first-party login stack:

- `codex_login`
  - browser login callback server
  - device-code login
  - API key login
  - logout
  - credential storage helpers
- `cli/src/login.rs`
  - direct-user login UX
  - headless fallback behavior
  - login-specific logging

## Required Boundary

RARA should distinguish:

1. Codex first-party/provider auth
   - This is the current target of the work.
   - It should align with Codex's browser/device/API-key login model.

2. MCP/appserver OAuth
   - This is a later, separate concern.
   - It should not be conflated with Codex provider login state.

They should not share one undifferentiated OAuth surface in TUI or CLI.

## Codex Alignment

RARA should align with Codex's first-party login flow and reuse Codex crates
where practical.

### Reuse Targets

Prefer reusing:

- `codex_login`
  - `run_login_server(...)`
  - `run_device_code_login(...)`
  - `login_with_api_key(...)`
  - `logout(...)`
  - `ServerOptions`
  - credential storage types and helpers where the dependency shape is
    acceptable
- small, isolated config/auth types from Codex when needed

### What Not To Reuse Blindly

Do not pull in Codex's entire config/runtime stack just to add login. In
particular, RARA should avoid tightly coupling to:

- Codex home-directory layout assumptions;
- Codex-specific TUI/login telemetry stack;
- Codex-only config indirection that RARA does not otherwise use.

RARA should wrap the reusable Codex login pieces behind its own thin auth
boundary.

## Target Architecture

### 1. First-Party Auth Manager

RARA's `OAuthManager` should stay as a thin adaptation layer around
`codex_login`, while continuing to expose a RARA-local surface for:

- browser login;
- device-code login;
- API key login;
- logout;
- durable credential loading/saving.

### 2. Runtime Entry Points

The Codex-auth path should be callable from:

- CLI:
  - `rara login`
  - `rara login --device-auth`
  - `rara login --with-api-key`
  - `rara logout`
- TUI:
  - a richer auth-mode picker that mirrors Codex:
    - browser login
    - device-code login
    - API key login
- future appserver/ACP integration:
  - can reuse the same provider auth state when the appserver path depends on
    Codex-authenticated provider access

## CLI Behavior

Codex-style behavior should be the baseline.

- `login`
  - starts browser login by default when local callback is practical.
- `login --device-auth`
  - uses device-code flow for SSH/headless environments.
- `login --with-api-key`
  - reads the API key from stdin, matching Codex's safer CLI pattern.
- `logout`
  - removes stored first-party auth credentials.

## TUI Behavior

The TUI `/login` path should remain an explicit mode picker similar to Codex:

- `Browser Login`
- `Device Code Login`
- `API Key Login`

The TUI should still keep provider login distinct from any future MCP/appserver
server login surface.

## Model and Reasoning Selection

Once Codex auth is available, RARA aligns with Codex's catalog-driven model
selection instead of maintaining a local hardcoded preset list.

### Model Source

RARA should prefer the same Codex model catalog metadata used by upstream
Codex:

- `codex_models_manager`
- `codex_protocol` model metadata exposed through `ModelPreset`

This means:

- the visible Codex model list comes from the active catalog;
- picker visibility respects `show_in_picker`;
- auth-aware filtering follows Codex's own catalog logic rather than a
  RARA-local allowlist.

### Picker Flow

The TUI `/model` flow for Codex behaves as follows:

1. If no Codex auth is available, open the Codex auth-mode picker.
2. If Codex auth is available, refresh/load the Codex model catalog and open
   the model picker.
3. After selecting a model:
   - if the model exposes more than one supported reasoning level, open a
     second picker for reasoning level;
   - if only one reasoning level is available, apply it directly and rebuild
     without an extra overlay.

### Persisted State

RARA's provider-scoped state for `codex` must now remember:

- `model`
- `reasoning_effort`
- `base_url`
- credential state

Switching away from and back to Codex should restore the remembered model and
reasoning effort together.

## Appserver / MCP Relationship

This feature is related to appserver support, but it is not the same feature.

The expected dependency order is:

1. Codex first-party auth parity
2. real ACP/appserver runtime
3. optional MCP/appserver server-scoped OAuth where needed

If later appserver work needs MCP/server-scoped auth, that should be specified
and implemented separately.

## Validation

Minimum validation for implementation:

- unit tests:
  - browser login callback handling
  - device-code path selection
  - API key login persistence
  - logout removes stored auth correctly
- CLI tests:
  - `login --with-api-key` requires stdin input
  - `login --device-auth` works in headless-friendly mode
- integration tests:
  - browser callback flow works with configured callback port/url
  - TUI and CLI both load the same saved auth state

## Current Checkpoint

Implemented in the current pass:

- browser login now uses `codex_login::run_login_server(...)`
- device-code login now uses `codex_login::request_device_code(...)` and
  `codex_login::complete_device_code_login(...)`
- API-key login now uses `codex_login::login_with_api_key(...)`
- logout now uses `codex_login::logout(...)`
- RARA still persists the resulting provider credential into its local config
- the TUI auth picker keeps the Codex-style selection-list UX
- Codex auth storage now lives under `~/.rara/codex-auth` instead of a
  project-local `.rara`
- the main RARA config now lives under `~/.rara/config.json`, with
  provider-scoped remembered state for multi-provider/model use
- the Codex runtime path no longer reuses the generic OpenAI-compatible
  `chat/completions` backend; Codex requests now target the Responses API at
  `/v1/responses`, matching upstream `codex-rs`
- the Codex model picker no longer uses a local hardcoded preset list; it now
  loads the active Codex catalog through `codex_models_manager`
- Codex reasoning effort is now a first-class provider-scoped setting instead
  of being squeezed into the existing boolean `thinking` flag
- the TUI Codex flow is now two-stage:
  - model picker first
  - reasoning-level picker second when the chosen model supports multiple
    efforts
- focused auth regression tests now cover:
  - stored API-key persistence
  - logout cleanup
  - credential loading precedence
  - auth-picker navigation
- focused Codex catalog tests now cover:
  - carrying selected reasoning effort into `/v1/responses`
  - opening the reasoning picker before rebuild for multi-effort Codex models
- initial snapshot coverage now exists for:
  - auth picker popup
  - queued follow-up preview
  - Updated Plan active-turn rendering

Still intentionally left open:

- fuller browser callback-path validation beyond the `codex_login` server bridge
- auth-mode-specific Codex endpoint selection:
  - ChatGPT/Codex login should not necessarily reuse the same endpoint as an
    OpenAI API key
- broader snapshot coverage for more TUI popups and transcript-heavy surfaces
- richer credential persistence semantics if Codex later requires more than the
  current stored access token/API key path

## Follow-Up Work

- Expand auth-path tests around browser callback handling and shared persistence.
- Align Codex endpoint selection with the active auth mode so OAuth-backed
  Codex sessions and API-key-backed Codex sessions do not share the wrong
  provider URL by accident.
- Expand snapshot coverage across more Codex-style TUI surfaces.
- Specify MCP/appserver OAuth separately if later appserver work truly needs it.
