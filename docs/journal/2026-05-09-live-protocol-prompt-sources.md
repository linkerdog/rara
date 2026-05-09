# Live Protocol Prompt Sources

## Context

Protocol prompt sources could be registered and converted into prompt-runtime
source objects, but the agent did not yet refresh the live registry during
request assembly.

## Changes

- Runtime bootstrap now owns a shared `PromptSourceRegistry` backed by the
  runtime event bus.
- Agents can attach that registry through `set_prompt_source_registry`.
- At the start of each user query, the agent atomically refreshes registry
  snapshots into `PromptRuntimeConfig::protocol_prompt_sources`.
- Turn-limited prompt sources are advanced under the same registry lock as the
  snapshot, so a `Turns(1)` source participates in one complete user query and
  is then expired without racing later registrations.
- Added regression coverage that verifies protocol prompt sources appear in the
  effective prompt and `/context` prompt-source view, remain available for the
  current query after expiration from the registry, then disappear on the next
  query refresh.

## Remaining Work

- Publish structured lifecycle events for prompt-source injection and
  expiration.
- Wire external appserver/ACP/Wire prompt-source controls to the shared runtime
  registry once those adapters expose typed prompt-source requests.
