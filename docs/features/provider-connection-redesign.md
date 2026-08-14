# Provider Connection And Model Selection

## Problem

Provider configuration had accumulated separate TUI commands and picker
paths for login, logout, base URLs, profiles, individual provider model
pickers, and a unified model picker. These paths duplicated provider-specific
state decisions in the TUI and made it unclear whether a provider was ready to
use or merely had a credential saved.

## Scope

The TUI has exactly two first-class provider entry points:

- `/connect` manages provider credentials and provider-specific connection
  settings.
- `/model` selects the active model from available providers.

The existing persisted configuration remains the compatibility source during
the transition. CLI `login`, `logout`, and `connect` commands are outside this
TUI command-surface contract.

## Non-Goals

- Changing the persisted credential format in this rollout.
- Removing the provider-specific backend implementations.
- Adding provider discovery beyond the existing catalog and API model refresh.
- Exposing a new public app-server protocol before its runtime projection is
  stable.

## Architecture

Runtime-owned provider state is the canonical source for presentation
surfaces. It distinguishes credential presence from runtime availability and
projects provider-scoped model catalogs. The TUI renders that projection and
submits explicit provider operations; it does not parse provider API responses
or decide connection state from configuration fields.

Until the projection fully replaces legacy state, an adapter may read
`provider_states`, `openai_profiles`, and Codex auth storage. That adapter is a
compatibility boundary, not a second canonical model.

## Contracts

### `/connect`

`/connect` opens the provider list. Selecting a provider opens either its
configuration flow or its management view.

- An unconfigured provider collects the required API key, OAuth method, or
  endpoint fields, verifies the configuration when practical, and refreshes
  its model catalog.
- A configured provider remains selectable for credential replacement,
  reconnect, and logout actions. It must not stop at an informational "already
  connected" notice; model selection remains in `/model`.
- API key, browser OAuth, device-code OAuth, endpoint URL, and profile details
  are `/connect` steps, not top-level commands.

### `/model`

`/model` is the sole model picker. It searches and groups models by provider
and only exposes models from providers that are currently available according
to the runtime projection. Selecting a model persists the provider/profile and
model together, then requests the normal session-stable backend rebuild.

When no provider is available, the picker presents `/connect` as the recovery
action instead of showing synthetic fallback models as selectable.

### Removed TUI Commands

The command palette must not expose `/login`, `/auth`, `/logout`, `/base-url`,
or `/models`. Their behavior is represented by `/connect` or `/model`.

### Availability States

The stable runtime projection distinguishes at least:

- `unconfigured`
- `credential_configured`
- `verifying`
- `available`
- `failed`

Credential presence alone is not presented as verified availability.

## Validation Matrix

| Contract | Verification |
| --- | --- |
| Command surface contains only the two provider entry points | Focused command parsing and palette tests |
| `/connect` opens provider setup | Focused TUI state/event test |
| `/model` opens the sole searchable model picker | Focused TUI state/event test |
| Removed commands are not accepted by the TUI parser | Focused command tests |
| Provider/model selection remains session-stable | Existing rebuild and runtime snapshot tests |

## Operational Notes

The current model catalog already treats static provider metadata as durable
and API `/models` results as runtime availability. The two-entry TUI contract
builds on that boundary; future runtime work must carry the provider
availability projection alongside model catalogs.

## Open Risks

- Legacy config carries both global and provider-scoped values, so migration
  must preserve precedence and existing credentials.
- Browser OAuth and device-code completion must refresh the provider projection
  before the model picker is shown.
- Custom OpenAI-compatible profiles need stable provider/profile identity in a
  future runtime projection.

## Source Journals

- `docs/journal/2026-08-14-provider-entrypoint-consolidation.md`
- `docs/journal/2026-08-04-provider-model-catalog.md`
