# DeepSeek Anthropic-Compatible Route

## Problem

`deepseek-flash` requests over `chat/completions` occasionally encode tool
calls as inline `<｜DSML｜tool_calls>...` markup inside the `content` delta
stream instead of the structured `tool_calls` field. The streaming client
only knew how to buffer a leading `<think>` block, so this markup (and its
JSON arguments) leaked into every live consumer — TUI, print, wire, exec,
ACP — before the final, fully-buffered response was re-scrubbed. Users saw
this as scrambled/out-of-order output mid-stream.

## Scope

- Route `deepseek-flash` specifically, and only when the configured
  `chat/completions` base URL resolves to DeepSeek's own official endpoint
  shape (`https://api.deepseek.com`, default port, no path beyond `/v1`), to
  DeepSeek's Anthropic Messages-compatible surface at
  `https://api.deepseek.com/anthropic`.
- `chat/completions`' streaming scrubber (`DeepseekTextStreamScrubber` in
  `src/llm/openai_compatible.rs`) is generalized in the same change to hold
  back inline DSML tool-call markup wherever it appears in the stream, not
  just a leading `<think>` block — this is the safety net for every other
  DeepSeek model and every custom/proxied `deepseek` endpoint.

## Non-Goals

- No other DeepSeek model (`deepseek-v4-pro`, reasoner variants) moves off
  `chat/completions` in this change; they are not verified against the
  Anthropic-compatible surface.
- No new native Anthropic Messages backend for actual Anthropic/Claude
  models. This is a DeepSeek-specific route reusing Anthropic's wire
  protocol, not a general Anthropic provider (tracked as an explicit gap in
  `docs/journal/2026-09-18-provider-registry.md`'s coverage comparison).
- Registry-model image/vision options, Files API upload, and account-token
  auth (`x-dsh-auth-token`) from DeepSeek's own reference harness are not
  implemented; only API-key auth (`x-api-key`) is supported.

## Architecture

`DeepseekAnthropicBackend` (`src/llm/deepseek_anthropic.rs`) wraps an
`OpenAiCompatibleBackend` as `fallback` and implements `LlmBackend`:

- `ask_with_context` / `ask_streaming_with_context` hit
  `POST {base}/v1/messages` directly and are the only methods with new
  logic.
- Every other trait method (`summarize*`, `classify_with_context`,
  `context_budget`, `cache_profile`, `request_cache_fingerprint`,
  `model_label`) delegates to `fallback`, so non-streaming and auxiliary
  calls keep using `chat/completions` unchanged.
- `billing_provider` is copied from `fallback.billing_provider()` at
  construction so streamed attempts price against the same tariff key as
  `chat/completions`, rather than reporting as a distinct/unpriced provider.

Routing lives in `src/runtime_context.rs`'s two backend-construction sites,
both calling `wrap_deepseek_anthropic_if_eligible`
(`llm::deepseek_anthropic::wrap_if_eligible`), which never fails: the one
fallible step (building the HTTP client) runs before `fallback` is moved
into the wrapper, so any failure there returns `fallback` unchanged instead
of losing it.

## Contracts

- **Eligibility**: `kind == OpenAiEndpointKind::Deepseek`, `model ==
  "deepseek-flash"` exactly, and the configured base URL is HTTPS, the
  default port, no credentials/query/fragment, and a root or `/v1` path on
  host `api.deepseek.com` (case-insensitive). Anything else stays on
  `chat/completions`.
- **Message shape**: internal `Message.content` blocks (`text`, `tool_use`,
  `tool_result`) already match Anthropic's content-block shape, so
  per-message conversion (`to_anthropic_message_content`) is close to
  identity. System history may be a plain string or an array of text blocks
  (compaction carry-over); both render into the top-level `system` field via
  the shared `extract_message_text`.
- **Tool-result adjacency**: not an identity conversion. Anthropic requires
  every `tool_use` in an assistant turn to have its `tool_result` in the
  literal next message, but the agent loop records one turn's parallel tool
  results as several separate consecutive `user` `Message`s (one
  `tool_result` block each — `execute_tool_calls`/`tool_result_message` in
  `src/agent/execution.rs` and `src/agent/planning.rs`), plus a trailing
  runtime continuation nudge, also `user`-role. `to_anthropic_messages`
  coalesces consecutive `user` messages into one before sending — never
  `assistant` messages, since `to_anthropic_message_content` always places a
  replayed `thinking` block first in its own message, and merging a later
  assistant message's blocks after an earlier one's would bury it there
  instead, breaking the thinking-signature replay contract above. It also
  mirrors `chat/completions`' `flush_missing_tool_results`: a `tool_use` id
  still unresolved when the next assistant message (or end of history) is
  reached gets a synthesized `is_error: true` `tool_result`, since an
  approval- or plan-exit-interrupted turn can abandon part of its batch with
  no result ever recorded, and `repair_tool_result_history` (the general
  repair pass) only runs at the start of a fresh user query, not on the
  approval-resume path. It also mirrors the *other* direction of
  `repair_tool_result_history`'s own repair (`src/tool_result/transcript.rs`):
  a `tool_result` block whose id is not currently pending — its real
  `tool_use` was already resolved, already flushed as a synthetic filler, or
  never existed — is dropped rather than passed through, since Anthropic
  rejects that too (`tool_use_id found in tool_result blocks ... without a
  corresponding tool_use block in the previous message`). Fixed in response
  to two real production 400s hit on long-running sessions with parallel
  tool calls; see the dated journal entries.
- **Thinking-signature replay**: verified live against `api.deepseek.com`
  — a tool-using turn's `thinking` block, including its `signature`, must be
  replayed on the next request whenever tools are active, or the API
  rejects the request with `400 ... thinking mode must be passed back`.
  This backend stores that block as `ContentBlock::ProviderMetadata{
  provider: "deepseek", key: "thinking", value: {thinking, signature} }` on
  responses and reconstructs it (placed first in the content array) on the
  next request. A session whose history was built on `chat/completions`
  (which stores reasoning as a plain `reasoning_content` string, no
  signature) has nothing to replay if it later switches to this backend
  with tools active — DeepSeek will reject that turn. This requires an
  active mid-session provider/model switch and is not otherwise mitigated.
- **Streaming event mapping**: `content_block_start`/`_delta`/`_stop` for
  `text`/`thinking`/`tool_use` blocks map onto `LlmStreamEvent::TextDelta`
  /`ReasoningDelta` and the final `ContentBlock` vocabulary, accumulated in
  the order blocks close (their natural generation order — no client-side
  reordering is needed the way `chat/completions`' DSML fallback needs it).
  Malformed protocol data (a `tool_use` block missing `id`/`name`, or
  accumulated `input_json_delta` fragments that don't parse as JSON) is
  propagated as an error rather than silently substituted, mirroring
  `chat/completions`' `parse_tool_arguments`.
- **Usage accounting**: DeepSeek's `message_delta.usage` has been observed
  carrying a complete snapshot, but Anthropic's own documented behavior only
  guarantees `output_tokens` there; usage from `message_start` and
  `message_delta` is merged field-by-field rather than the later event
  fully overwriting the earlier one.

## Validation Matrix

Live-verified against `api.deepseek.com` with an account key before and
after implementation:

- Non-streaming and streaming requests against `deepseek-flash` (plain
  text, and with `tools`) succeed and match the documented Anthropic
  Messages event shapes.
- A tool-call round trip (assistant `tool_use` → user `tool_result`) works
  when the `thinking` block is replayed, and fails with the documented 400
  when it is omitted (reproduced deliberately to confirm the requirement).
- End-to-end through the real `print` consumer binary: a `read_file`
  tool-using turn round-trips cleanly (tool call, result, thinking-signature
  replay, final answer), no leaked markup.

Unit coverage: `src/llm/deepseek_anthropic.rs` (base-URL eligibility shape,
message conversion including compaction's array-shaped system content and
thinking-block reconstruction, block assembler seeding/validation/error
propagation, usage merge, usage parsing) and `src/llm/tests.rs` (the
generalized `chat/completions` DSML stream buffering).

## Open Risks

- The registry-model output-token cap and `temperature`/`top_p` are carried
  in from the registry model's own settings at the `runtime_context.rs`
  call site (since `OpenAiCompatibleBackend` exposes no getters for them);
  the non-registry `openai-compatible` construction path has no such
  settings to carry, so this backend falls back to a fixed 256k output cap
  there, matching the DeepSeek reference harness's own default.
- `reasoning_effort` is sent as `output_config.effort` (matching DeepSeek's
  own reference harness's documented request shape) verbatim from
  configuration; the exact accepted value vocabulary for `deepseek-flash`
  specifically (as opposed to `chat/completions`' v4/reasoner-only
  normalization) is not verified beyond "the API accepts it without
  erroring."
- The chat/completions DSML scrubber fix does not hold back an "orphaned"
  closing fragment (a stray `</｜DSML｜invoke>` with no matching open tag)
  appearing mid-stream — only a complete tool-call block. The pre-existing
  lenient end-of-stream fallback still covers that rarer, already-malformed
  case.
- `resume_after_plan_approval_with_feedback_events`/
  `reject_pending_plan_approval` (`src/agent/planning.rs`) resume the agent
  loop directly, bypassing `query_inner`'s `repair_tool_result_history`
  call. An approval- or plan-exit-interrupted turn's earlier-resolved
  tool results (from tool calls processed before the one needing approval,
  in the same batch) are therefore lost outright, not just delayed —
  `flush_missing_tool_results` here only stops the *pairing* from producing
  a malformed wire request; it synthesizes a filler, it does not recover
  the original result data. Cross-cutting agent-loop issue, not specific to
  this backend; out of scope here.

## Source Journals

- `docs/journal/2026-09-23-deepseek-anthropic-route.md`
- `docs/journal/2026-09-23-deepseek-anthropic-tool-result-adjacency.md`
- `docs/journal/2026-09-24-deepseek-anthropic-orphaned-tool-results.md`
