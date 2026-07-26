# Subagent Provider Routing

## Summary

Added runtime-owned provider and model routing for subagents. The
implementation follows the same boundary used by Codex model providers and
managed multi-agent systems: tool calls and TUI surfaces carry provider targets,
while runtime assembly resolves those targets into concrete backend handles.

## What Changed

- Agent definitions may now set an optional `provider` frontmatter field in
  addition to `model`.
- `model: provider:model-id` is supported as a compact form when an agent file
  should select another configured provider.
- Bare `model` values continue to mean "use the current provider with this
  model".
- `model: inherit` and omitted values inherit the parent backend.
- Configured provider states with `base_url` and `model` are routable as custom
  OpenAI-compatible provider instances, so users can add provider IDs such as
  `groq-fast` or `xai-large` without a new native backend.
- `team_create` task entries accept optional `provider` and `model` fields so a
  batched delegation can run heterogeneous workers.
- Subagent result payloads and persisted runtime metadata now carry the resolved
  provider and model.

## Trade-Offs

RARA does not add a new provider dependency or copy Rig's generic agent
abstraction. Rig's `CompletionModel` design is useful as a provider-agnostic
request shape, but its associated types make dynamic runtime routing an
application-level responsibility. RARA keeps the existing `LlmBackend` trait and
adds a small resolver that builds another backend from the current config
snapshot. The provider-state path intentionally treats configured custom
providers as OpenAI-compatible until native protocol backends are added.

The compact `provider:model` form avoids slash parsing because provider model
IDs can include slash separators.

## Verification

- `cargo test tools::agent::tests::`
- `cargo test runtime_context::tests::config_subagent_backend_resolver_builds_configured_provider_state`

## Follow-Up

- Expose configured provider/model choices in a runtime status surface for
  agent definitions.
- Add API-backed model catalog loading for all connected providers, then use it
  to validate subagent overrides before model execution.
- Add native non-OpenAI-compatible provider adapters where the protocol differs
  materially from chat completions.
