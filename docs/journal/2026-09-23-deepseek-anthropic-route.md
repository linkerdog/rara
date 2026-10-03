# DeepSeek Flash Streaming Fix And Anthropic-Compatible Route

## Summary

Fix `deepseek-flash` output looking scrambled/out of order during streaming,
at two layers. The canonical contract is
[deepseek-anthropic-route.md](../features/deepseek-anthropic-route.md).

## Root Cause

`deepseek-flash` sometimes encodes tool calls as inline
`<｜DSML｜tool_calls>...` markup inside the `content` delta stream instead of
the structured `tool_calls` field. `DeepseekTextStreamScrubber`
(`src/llm/openai_compatible.rs`) only buffered a leading `<think>` block, so
this markup — wherever it appeared in the stream — leaked live into every
consumer (TUI, print, wire, exec, ACP) before the final, fully-buffered
response was re-scrubbed.

## Reference Review

- `deepseek-ai/deepseek-harness` (GitHub): `packages/llm/llm-deepseek`'s
  README documents DeepSeek's own official adapter for `deepseek-flash`
  using an Anthropic Messages-compatible endpoint
  (`https://api.deepseek.com/anthropic`, `x-api-key` auth,
  `anthropic-beta: files-api-2025-04-14`), not `chat/completions` or the
  OpenAI-style `/responses` endpoint documented separately at
  api-docs.deepseek.com.
- `docs/journal/2026-09-18-provider-registry.md`'s coverage comparison
  already flagged native Anthropic Messages support as "explicit follow-up,
  not mapped silently to Chat Completions" — this change is that follow-up,
  scoped to DeepSeek's own reuse of the protocol rather than a general
  Anthropic provider.
- Live-verified directly against `api.deepseek.com` with an account key
  before writing any client code: request/response and SSE event shapes,
  auth header, and the `thinking`-signature replay requirement (reproduced
  the `400 ... thinking mode must be passed back` error deliberately, then
  fixed it).

## Plan And Implementation

1. Generalize the chat/completions streaming scrubber
   (`DeepseekTextStreamScrubber`) to buffer inline DSML tool-call markup
   wherever it appears in the stream, not just a leading `<think>` block —
   composed as two independent stages (`DeepseekLeadingThinkStage`,
   unchanged; new `DeepseekDsmlStage`) so the existing think-vs-literal
   ambiguity handling stays exactly as tested.
2. Add `DeepseekAnthropicBackend` (`src/llm/deepseek_anthropic.rs`), wrapping
   `OpenAiCompatibleBackend` as a fallback for every `LlmBackend` method
   except the main streaming request. Wire it in at both
   `runtime_context.rs` backend-construction sites, gated on the model
   being exactly `deepseek-flash` and the configured base URL resolving to
   DeepSeek's exact official endpoint shape.
3. End-to-end verification through the real `print` consumer binary against
   the live API: a `read_file` tool-using turn round-trips cleanly,
   including the thinking-signature replay.

## Review Findings (Copilot, PR #896)

A first-pass automated review flagged several real gaps in the initial
Anthropic backend, all fixed in follow-up commits on the same PR:

- `content_block_start` payload fields (initial text/thinking/`tool_use`
  input) were ignored rather than seeded, and a `tool_use` block missing
  `id`/`name` silently became an empty-argument call instead of an error —
  fixed by seeding from the start payload and validating required fields.
- Truncated/malformed accumulated tool-call JSON silently became `{}`
  instead of propagating a decode error, unlike `chat/completions`' own
  `parse_tool_arguments` — fixed to propagate.
- The base-URL eligibility check validated only the host, so a differently
  shaped URL on the same host (custom path, non-default port, credentials)
  would still be silently rerouted — fixed to require the exact official
  shape (HTTPS, default port, no credentials/query/fragment, root or `/v1`
  path).
- Registry-model `max_tokens`/`temperature`/`top_p` and the configured
  `reasoning_effort` were dropped rather than forwarded — fixed by carrying
  them in from the `runtime_context.rs` call site (the wrapped backend
  exposes no getters for its own construction inputs) and sending effort as
  `output_config.effort`, matching the reference harness's documented
  request shape.
- The billing provider label used for cost accounting didn't match
  `chat/completions`' identity, which would have reported all Anthropic-path
  usage as unpriced — fixed to reuse the wrapped backend's own
  `billing_provider()`.
- `message_delta.usage` was treated as the complete final usage, discarding
  `message_start`'s input/cache accounting — fixed to merge usage
  field-by-field across both events.
- System history that is an array of text blocks (compaction carry-over),
  not a plain string, was silently dropped — fixed to reuse the shared
  `extract_message_text` helper other backends already use for this.

One finding was investigated and declined: that `ContentBlock::ProviderMetadata`
being excluded from `assistant_turn_history_message`'s persistence check
could lose a `thinking`-only turn's signature. Broadening that check to
`agent.rs` regressed an existing, deliberately tested safety net
(`reasoning_only_turn_is_not_persisted_as_empty_assistant_message`): a turn
with only reasoning and no visible text or tool call is discarded and the
loop force-continues rather than being persisted at all, so its
signature is never needed for replay — it never enters history. No live
sample produced a `thinking`-only response with no text/tool call either.
