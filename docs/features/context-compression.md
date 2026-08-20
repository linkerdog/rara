# Context Compression

## Problem

RARA already compacts long conversations, but the current compaction output is still a generic
summary blob. That makes the result less stable than Claude Code style context compression, where
important state is preserved through a predictable structure instead of depending on free-form
summaries.

For long coding sessions, this increases the risk of losing:

- the exact user objective;
- concrete file paths already inspected or edited;
- current plan state;
- pending approvals or questions;
- unresolved risks and the immediate next action.

## Scope

- The compact prompt contract and default compression schema.
- The compacted history marker that gets written back into `Agent.history`.
- Recent-file carry-over and compact observability in `/status`.
- Limited recent-file excerpt carry-over for the most recent `read_file` results.
- Per-request tool-result projection before model calls.
- Compact-boundary metadata persistence across session save/restore.
- Focused tests for compaction prompt and stored summary shape.

## Non-Goals

- Full recent-file snippet reattachment after compaction.
- Token-cache aware prompt reuse.
- Provider-specific remote compaction APIs.
- Claude-style cache-edit microcompaction for providers that do not expose a
  cache-edit API.

## Architecture

### 1) Compact Planning Boundary

- Compaction planning must operate on API-round groups rather than raw history item counts.
- An API-round group starts when a new assistant response begins and includes the user tool results
  that answer that assistant response.
- The planner should summarize an older prefix and retain a recent suffix only at group
  boundaries, so assistant `tool_use` items are not separated from their matching user
  `tool_result` items.
- The retained suffix is token-budget aware. The default target is a fraction of the current
  compact threshold, while still retaining at least the newest API-round group.
- Raw item-count heuristics such as "summarize the oldest 80%" are fallback-quality behavior and
  should not be used as the normal planning strategy.
- Runtime partial compaction accepts explicit `from` / `up_to` message indexes only when both
  indexes align with API-round group boundaries. It replaces just that selected range with compact
  boundary metadata, structured summary, and source-aware carry-over, then preserves the unchanged
  prefix and suffix.
- If the summary model returns a structured context-window error while building a compaction
  summary, the runtime retries by dropping the oldest API-round group from the summary input. It
  keeps doing this only for context-window failures; authentication, rate-limit, network, and other
  provider errors keep the normal failure path.
- Post-compact carry-over sources expose stable descriptors such as
  `history.compaction.summary`, `history.compaction.boundary`, and
  `history.compaction.recent_files`. The actual compacted history stores each
  carry-over block as model-readable text plus a typed source item, so normal
  model calls keep a readable prompt while `/context` and future protocol
  subscribers can consume stable descriptors from the same runtime artifact.
  `/resume` shows the latest compaction boundary/range/token metadata in the
  thread picker.

### 2) Structured Compact Prompt

- The default compact prompt should require a stable markdown schema instead of a generic prose
  summary.
- The first phase keeps the schema simple and directly usable by the next turn.

### 3) Required Compression Sections

The default compact output should preserve, in order:

1. `User Intent`
2. `Constraints`
3. `Repository Findings`
4. `Files Touched Or Inspected`
5. `Plan State`
6. `Pending Interactions`
7. `Unresolved Risks`
8. `Next Best Action`

### 4) Stored History Shape

- After compaction, RARA should store a clearly labeled structured summary in history instead of a
  generic `"SUMMARY OF PREVIOUS CONVERSATION"` marker.
- The stored marker should make it obvious to both runtime and future debugging that this is a
  compaction artifact with a stable schema.
- RARA should also write a compact boundary record ahead of the summary so later tooling can detect
  compaction boundaries without scraping free-form summary text.
- Compact boundary metadata should also be mirrored into persisted session state so resume flows and
  status views can recover the latest compaction boundary without reparsing full history.

### 5) Post-Compact Carry-Over Stages

Post-compact history is assembled in ordered stages:

1. compact boundary metadata;
2. structured summary of the summarized API-round prefix;
3. source-aware carry-over such as recent files and recent file excerpts;
4. retained recent API-round suffix.

Future memory, hook, skill, MCP, and runtime-state reinjection should plug into the source-aware
carry-over stage by adding typed source items with deterministic source descriptors. They must not
be special-cased inside the split planner. This mirrors Claude Code's separation between the summary
replacement and the post-compact attachments that restore current working context.

The generic source item shape is:

```json
{
  "type": "compaction_carry_over",
  "kind": "compacted_memory",
  "label": "Memory Carry-over",
  "source_descriptor": "history.compaction.memory",
  "detail": "Short context shown in /context and MemorySelection",
  "inclusion_reason": "Why this item was carried forward"
}
```

`source_descriptor` must stay under the `history.compaction.*` namespace. The
same typed item can be used for memory, hook, skill, MCP, and runtime-state
carry-over classes as those producers are added.

The current implementation emits:

- `history.compaction.memory` for retrieved memory candidates that were
  available before compaction;
- `history.compaction.skills` for skills invoked before compaction;
- `history.compaction.hooks` for hook retain hints from compacted history;
- `history.compaction.mcp` for MCP retain hints from compacted history.

Hook and MCP retain hints use `type = "compaction_retain_hint"` in the
pre-compact history and must provide a stable source descriptor under `hook.*`
or `mcp.*`. Runtime-state producers should use the same generic item shape and
deterministic descriptor namespace.

### 6) Prefix Stability

- Context cache reuse depends on stable prompt prefixes. Compaction must not reorder stable context
  sources as a side effect of summarizing history.
- Post-compact carry-over stages are append-only and ordered by source class. New source classes
  must be added at an explicit slot instead of being mixed into existing free-form text.
- Within a source class, entries should use deterministic ordering when the source is stable. Runtime
  recency ordering is acceptable only for explicitly recent artifacts such as recent files and file
  excerpts.
- The retained recent API-round suffix always comes after compact metadata, summary, and carry-over
  sources. This keeps deterministic system/context material before volatile conversation history.

### 7) Tool Result Projection

Tool-result projection is a read-time microcompact pass that runs before the
model request and before summary autocompaction pressure becomes the only
option.

The projection pass:

- operates on the request copy of `Agent.history`, not the persisted transcript;
- targets only high-volume tools such as shell, file read/search, web, and edit
  tools;
- keeps the most recent compactable tool results verbatim;
- identifies the active turn from the latest real user-text request rather than
  counting `tool_result` messages as new user turns;
- reduces prior-turn results first to reference summaries that retain tool
  identity, relevant input, and any persisted full-result path;
- when pressure remains, reduces older active-turn results to bounded semantic
  evidence: file/range/content excerpts for reads, query/scope/count/sample for
  searches, and command/outcome/head-tail evidence for shell calls;
- uses a minimal reference-only fallback only when semantic summaries still
  exceed the request budget; it never replaces active evidence with an
  unqualified generic cleared marker;
- never changes `tool_use` / `tool_result` pairing or removes the block itself;
- never rewrites stable system, tool schema, skill, or memory prompt prefixes.

This is the safe baseline for DeepSeek and OpenAI-compatible providers that
have automatic prefix caching but no cache-edit API. It reduces volatile
history size while preserving the full local transcript for restore,
distillation, and debugging.

The runtime exposes projection as a transient status event when old tool
results are projected out of a model request and as a structured context
observability view after the request. The view records the policy, original
chars, projected chars, saved chars, summarized result count, reference-only
result count, active-turn retained result count, and retained result count.
The legacy cleared count remains for compatibility and should stay zero unless
a legacy input already contains the old marker. It is read-only accounting over
the request projection and must not be used to rewrite persisted transcript
history.

The same structured projection report should be reusable by future OpenTelemetry
exporters. `/context` remains the local debugging surface, while OTEL should
export session-scoped events, counters, histograms, and trace context from the
same runtime data model. Context compression must not introduce a separate
display-only accounting path that would drift from exported telemetry.

The context observability model also carries cache usage, compaction summary
counters, and retrieval accounting so `/context`, ACP/Wire, and future OTEL
exporters can share the same event shape instead of parsing TUI strings.

Agent turn trace is part of the same observability model. Each latest turn
records whether the model produced visible text, reasoning, stream deltas, tool
calls, a persisted assistant message, and the loop outcome or continuation
phase chosen by the runtime. This is the debugging surface for "thinking-only"
stalls: a trace with `reasoning_only = true`, `tool_call_count = 0`, and no
continuation phase points to a runtime decision bug; a trace with missing tool
calls after DSML text points to provider parsing; a trace with a continuation
phase but no later response points to the next model request or transport path.

Provider-specific cache-edit microcompaction is an optional branch. It is now
represented in the runtime policy and observability model, but no backend
executor applies cache edits yet. The policy is intentionally provider-gated:
cache-edit eligibility is copied only from `LlmBackend::cache_profile()` and is
not inferred from OpenAI-compatible request shape alone.

### 8) Provider Cache Profiles

Model backends expose a cache profile with these independent capabilities:

- `automatic_prefix_cache`: repeated prompt prefixes may be cached by the
  provider without request parameters.
- `cache_usage_accounting`: usage metadata can report cache hit/miss tokens.
- `cache_edit`: the provider can delete or edit cached content without
  rewriting the local prompt content.
- `cache_retention_control`: the request API supports explicit cache retention
  controls.

DeepSeek is modeled as automatic prefix cache plus usage accounting, with no
cache edit and no retention control. OpenAI-compatible custom endpoints default
to no declared cache capability unless RARA has a provider-specific contract.

Compression logic must choose behavior from the cache profile:

- no `cache_edit`: use projection and ordinary compaction only;
- `cache_edit`: mark the request eligible for a provider cache-edit pass while
  continuing to preserve local messages; until a backend implements the actual
  executor, `cache_edit_applied` remains false;
- no `cache_retention_control`: do not inject provider-specific retention
  parameters.

### 9) Memory Placement

RARA has two memory classes with different cache behavior:

- Stable workspace memory, such as `.rara/memory.md`, is a prompt source. It
  belongs with stable instructions and should keep deterministic source order.
- Retrieved memory from session, thread, workspace, vector, or hybrid search is
  volatile per-turn context. It should be selected by `MemorySelection` and
  injected after stable prompt material, compacted carry-over, and the retained
  history projection, close to the latest user request.

Retrieved memory must not be prepended to the stable system prompt or inserted
before tool schemas. Doing so would make automatic prefix caches less useful for
providers such as DeepSeek. The current runtime injects selected retrieved memory
into the latest user message and does not persist that injected block to
`Agent.history`; this is the correct baseline until RARA has a first-class
attachment carrier for volatile context.

When future models are added, memory placement must follow the provider cache
profile:

- providers without cache edit: keep retrieved memory in the volatile suffix;
- providers with cache edit: cache-edit may optimize old tool results, but
  retrieved memory still remains per-turn volatile context unless explicitly
  promoted to workspace memory;
- providers without cache usage accounting: do not infer cache hit quality from
  memory placement alone.

### 10) Dedicated Compact Worker

Compaction can be executed by a dedicated internal worker, but it should not be exposed as a normal
model-callable sub-agent tool. Compact is a runtime lifecycle operation, not delegated task work.

The compact worker should receive a structured request:

- compact instruction;
- summarized API-round prefix;
- retained suffix plan;
- stable source descriptors for memory, hooks, skills, MCP, and runtime state;
- token budget and retry limits.

It should return a structured result:

- summary markdown;
- extracted carry-over items grouped by source class;
- warnings such as prompt-too-long truncation or schema drift;
- model usage, latency, and cache metrics when available.

The worker may use an auxiliary model for low-risk runtime reasoning that supports the main agent
turn but does not answer the user directly. This is intended for compression, context routing,
classification, and similar deterministic helper work where a smaller or cheaper model is
acceptable. It must not change the configured main chat model, tool policy, or conversation
ownership.

This mirrors Gemini's use of dedicated `chat-compression-*` model configs. The routing rule is:

- prefer an explicitly configured `auxiliary_model` for helper work;
- otherwise use a provider-specific lite model only when it can be derived conservatively;
- if no lite model is configured or derivable, use the main model;
- if an auxiliary/lite request fails because the provider does not support it, retry with the main
  model instead of failing compaction;
- for Codex/Responses streaming, classify `response.failed.error.code` structurally and treat
  `context_length_exceeded` as a context-window failure, matching Codex's upstream behavior;
- do not retry with the main model for authentication, rate-limit, network, or other provider
  failures because those are not auxiliary-model selection failures;
- when the helper model has a known smaller context window than the main model, compute compaction
  thresholds against the smaller helper budget so helper prompts are planned conservatively instead
  of relying on provider error-message parsing;
- persist metrics in a way that lets `/status` and `/context` distinguish main-model calls from
  auxiliary-model calls.

These controls are runtime-plane behavior, not TUI-only behavior. A compaction request should carry
the selected main model, optional auxiliary model, budget source, and trigger reason through the
shared request path. The resulting runtime event should report the model actually used, fallback
reason when fallback occurred, token usage/cache metrics when available, and any warning that should
be visible in `/status` or `/context`. TUI surfaces render those events; they must not infer routing
or fallback behavior from display text.

The first visible routing surface is intentionally read-only:

- `/status` shows the effective main model, auxiliary model, source, and route.
- `/context` shows the same main/auxiliary route near the context usage summary.
- Explicit `auxiliary_model` config wins over inference.
- DeepSeek OpenAI-compatible endpoints may conservatively infer `deepseek-v4-flash` from
  `deepseek-v4-pro`.
- Providers without an explicit or inferred helper route report `fallback`, meaning helper work uses
  the main model.

The parent agent should only apply the returned result through the post-compact assembly pipeline.
The worker transcript must not be appended to the parent conversation. This keeps prefix order
stable and preserves a clear boundary between main task history and compaction implementation
details.

This is similar in spirit to sub-agent isolation, but its storage model is different:

- normal sub-agents are child threads with task/result lineage;
- compact workers are lifecycle jobs attached to one compaction event;
- only the compact event, summary, carry-over, metrics, and warnings are persisted.

## Contracts

### 1) Preservation Rules

- Preserve the current objective as close to the user's wording as practical.
- Preserve concrete file paths when they were already inspected or edited.
- Preserve a small amount of recent `read_file` content so the next turn does not depend only on file
  names.
- Preserve the current plan and any pending approval or request-user-input state.
- Preserve the immediate next useful action instead of ending with a vague recap.
- Preserve tool-use integrity by cutting compacted and retained history only at API-round
  boundaries.

### 2) Failure Tolerance

- If the model returns imperfect formatting, compaction still succeeds.
- The structure is a prompt contract, not a hard parser contract in the first phase.

## Validation Matrix

- `cargo check`
- focused prompt tests for the default compact schema
- focused agent tests ensuring manual compaction stores the structured marker

## Open Risks

- Recent-file excerpt carry-over is still limited to recent `read_file` results; `grep` and
  `search` evidence are not yet restored the same way.
- Token accounting still relies on full-history re-estimation at some boundaries.
- Session restore mirrors compact boundary metadata, but it does not yet persist the full recent-file
  excerpt payload separately from history.
- Partial compaction currently exists as a runtime entrypoint. TUI, ACP, and Wire command surfaces
  still need to expose the range-selection workflow.

## Source Journals

- [2026-04-19-context-compression](../journal/2026-04-19-context-compression.md)
- [2026-05-06-prefix-stable-microcompact](../journal/2026-05-06-prefix-stable-microcompact.md)
