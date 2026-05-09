# Protocol Prompt Runtime Sources

## Context

Protocol prompt-source registration already retained runtime-control
provenance in `PromptSourceRegistry`, but the prompt runtime still had no
typed source variant for adapter-provided prompt material.

## Changes

- Added `PromptSourceKind::ProtocolPromptSource`.
- Added `PromptRuntimeConfig::protocol_prompt_sources` as the structured
  runtime input slot for protocol-managed prompt material.
- Render protocol prompt sources in a dedicated dynamic
  `protocol_prompt_sources` section so the existing workspace instruction,
  workspace memory, skills, runtime environment, and append-prompt ordering
  stays stable.
- Added `ProtocolPromptSourceSnapshot::to_prompt_source()` and
  `PromptSourceRegistry::list_prompt_sources()` to bridge registry snapshots
  into prompt-runtime source objects without exposing registry internals.
- Added focused tests for prompt rendering and registry-to-runtime conversion.

## Remaining Work

- Wire live `PromptSourceRegistry` snapshots into agent request assembly so
  ACP/Wire/appserver prompt-source registrations automatically populate
  `PromptRuntimeConfig::protocol_prompt_sources`.
- Add lifecycle events around snapshot injection once the live bridge exists.
