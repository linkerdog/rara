---
name: context-cache-management
description: Extend RARA context compression, provider cache behavior, memory placement, or model context budgeting while preserving stable prompt prefixes.
---

# Context Cache Management

Use this skill when changing RARA's context compression, tool-result projection,
provider cache handling, memory placement, context budget calculation, or
model-provider integration.

## Core Rules

- Treat the local transcript as source of truth. Request-time projection may
  shrink model input, but it must not rewrite persisted history unless the user
  explicitly asked for lifecycle compaction.
- Preserve `tool_use` / `tool_result` pairing. Do not remove a block from only
  one side of the pair.
- Keep stable prompt prefixes stable:
  1. system prompt
  2. tool schemas
  3. stable skills and project memory
  4. compacted history and carry-over
  5. retrieval and volatile recent context
  6. latest user input
- Keep retrieved memory in the volatile suffix unless it has been explicitly
  promoted to a stable workspace memory prompt source.
- Never infer cache-edit support from OpenAI-compatible request shape. Cache
  editing is a provider capability, not a protocol default.
- Do not inject provider-specific cache-retention parameters unless the backend
  explicitly declares retention control support.

## Provider Cache Capability Checklist

When adding or updating a model backend, declare these capabilities separately:

- `automatic_prefix_cache`: repeated prompt prefixes may be cached by the
  provider without request parameters.
- `cache_usage_accounting`: usage metadata reports cache hit/miss tokens.
- `cache_edit`: the provider can delete or edit cached content without changing
  local prompt content.
- `cache_retention_control`: request parameters can control cache lifetime.

DeepSeek is the reference example for automatic prefix cache with usage
accounting but without cache edit or retention control.

## Choosing A Compression Strategy

- If `cache_edit = false`, use request projection and ordinary history
  compaction. This reduces input size but may reduce prefix-cache hits after the
  edited point.
- If `cache_edit = true`, add a provider-specific pass that queues cache edits
  while leaving local messages unchanged.
- If the cache is likely cold, content projection is acceptable because the
  provider would rewrite the prefix anyway.
- If the cache is warm and no cache-edit API exists, prefer preserving recent
  raw turns and compacting only older volatile tool results.

## Memory Placement

- Stable workspace memory belongs with prompt sources and must render in a
  deterministic order.
- Retrieved memory belongs near the latest user request because it is
  query-dependent and changes turn by turn.
- Do not put retrieval results before tool schemas, system guidance, or stable
  project memory.
- If retrieved content becomes durable policy or project knowledge, promote it
  into stable workspace memory instead of repeatedly injecting it as volatile
  retrieval context.

## Tests To Add

For projection or microcompact changes:

- Unit-test the projection helper directly.
- Assert the original messages are unchanged.
- Assert recent compactable tool results remain visible.
- Assert non-compactable tool results remain unchanged.
- Add one agent-level test proving the model request is projected while
  `Agent.history` still contains the original tool result content.

For provider cache capability changes:

- Add a focused backend test for the declared `ProviderCacheProfile`.
- Add usage parsing tests when the provider reports cache hit/miss tokens.

For observability changes:

- Prefer structured projection reports over scraping status text.
- Keep transient status events short; put detailed per-request accounting in
  `/context` or another structured context surface.
- Keep the same report shape OTEL-ready. Local `/context` output and future
  telemetry exporters should read from the same data model.

## Common Mistakes

- Replacing old tool results inside the persisted transcript during a normal
  model request.
- Treating `prompt_cache_retention` or cache-edit fields as generally available
  across OpenAI-compatible providers.
- Adding dynamic per-turn retrieval before stable system/tool/skill context.
- Treating retrieved memory and workspace memory as the same cache-stability
  class.
- Clearing all old tool results and leaving the model with no working evidence.
- Reporting cache behavior from display text instead of structured provider
  usage fields or explicit backend capability.
