# Prefix-Stable Microcompact

## Context

DeepSeek exposes automatic prefix caching and cache hit/miss usage fields, but
does not expose a Claude-style cache-edit API or explicit cache-retention
controls. RARA therefore should not model OpenAI-compatible providers as if they
all support remote cache editing.

## Decision

Added the first provider-neutral microcompact slice as a request projection:

- keep the full transcript unchanged;
- project older compactable tool results out of the model request when the
  per-request tool-result budget is exceeded;
- keep recent compactable tool results verbatim;
- leave stable prompt prefixes, tool schemas, skills, and memory ordering alone;
- represent provider cache behavior as explicit capability flags.

## Implementation Checkpoint

- Added `ToolResultProjectionPolicy` and `project_tool_results_for_context`.
- Wired the projection pass into model request assembly before adding the stable
  system prompt.
- Added `ProviderCacheProfile` with DeepSeek configured as automatic prefix
  cache plus usage accounting, without cache edit or retention control.
- Added focused tests for projection behavior, transcript non-mutation, and
  DeepSeek cache profile.

## Follow-Up

- Add `/context` observability after the runtime context event model can carry
  per-request compression reports.
- Keep the same projection report reusable for future OTEL exporters, so local
  `/context` output and remote telemetry share one accounting source.
- Add provider-specific cache-edit microcompact only after a backend declares
  `cache_edit = true`.
- Add configurable per-tool projection budgets once tool result size policies
  move out of constants.
