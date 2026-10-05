# Provider Connections And Configured Models

## Scope

This supplements the [provider connection contract](../features/provider-connection-redesign.md)
and consumes the runtime-owned [provider registry](../features/provider-registry.md).

## PM-01: Connection Target Ownership

`/connect` includes enabled configured providers after the legacy provider
families. Each row shows its name, stable ID, and configured/key-required state.
These labels describe local configuration; they do not assert a successful API probe.

Selecting a configured provider opens a masked API-key editor. The selected
provider ID is captured when the editor opens. Later list navigation cannot
redirect the key to another provider. Enter saves the credential separately,
preserves the current model, and points the user to `/model`. Empty input is
rejected; Esc dismisses without saving. Config/environment credential overrides
are reported rather than silently replaced.

## PM-02: Model Identity And Availability

`/model` includes configured models from available providers, grouped by provider
name. Labels use `models[key].name`; selection uses the provider ID and model map
key. The backend receives `models[key].id` when specified. Slashes within the
model ID are preserved. A credential on one compatible provider does not make
another provider's models available.

Selecting a model requests the existing backend rebuild. Busy-session mutation
rules remain unchanged. The selected pair is saved as recent state without
rewriting the source configuration. On restart, an explicit configured default
has precedence over recent state.

## Verification

- Config tests cover merge/filter/credential precedence and persistence.
- TUI tests cover immutable credential targets, masked input, model labels,
  availability, and selection without changing unrelated provider credentials.
- Runtime HTTP capture covers the selected wire ID, exact API root, and limits.

## Source Journal

- [Provider registry rollout](../journal/2026-09-18-provider-registry.md)
