# Anchor Compaction's Token Estimate to Real Provider Usage

## Summary

A real production 400 from `api.deepseek.com/anthropic`, reported after
[2026-09-24-deepseek-anthropic-context-budget-reservation.md](2026-09-24-deepseek-anthropic-context-budget-reservation.md)
and its fail-loud follow-up merged — confirming the `max_tokens`/window
accounting fixed there was correct (the request's completion budget
matched exactly what was reserved), but a different, upstream problem
still let history grow past the point a request could fit at all:

```
This model's maximum context length is 1048576 tokens. However, you
requested 1049096 tokens (793096 in the messages, 256000 in the
completion).
```

`793,096 + 256,000 = 1,049,096`, only 520 tokens over the window — but
already past the compaction threshold
(`1,048,576 - 256,000 - 8,192 slack = 784,384`) by about 8,712 tokens,
meaning compaction should already have triggered before this request went
out.

## Root Cause

`Agent`'s compaction-trigger check (`compact_history_with_reporter` in
`src/agent/compact/main.rs`) compares `compact_state.estimated_history_tokens`
against the threshold. That estimate is built by
`estimate_history_tokens`/`record_history_message_tokens`
(`src/agent/compact/helpers.rs`), which counts every message against a
single, hardcoded `tiktoken_rs::cl100k_base()` tokenizer — OpenAI's
vocabulary, the only one this crate has a local BPE for, used
unconditionally regardless of which provider is actually being talked to.

DeepSeek's own tokenizer (used by both its `chat/completions` and
Anthropic-compatible surfaces) disagrees with `cl100k_base`, especially on
code and tool-call JSON. At small scale the gap is invisible; at the edge
of `deepseek-flash`'s 1,048,576-token window, a systematic few-thousand-
token undercount is exactly enough to let the local estimate say
"still under threshold" for history DeepSeek's real tokenizer counts as
already over it. The estimate is also never corrected against reality —
nothing in the agent loop ever compares it to a provider-reported `usage`
figure, so the drift, once introduced, persists and can compound over a
long session.

## Fix

Compared against DeepSeek's own reference harness
(`deepseek-ai/deepseek-harness`, `packages/llm/token-meter/src/projection.ts`):
its `ContextPressureProjection` doesn't try to precisely re-tokenize the
whole conversation locally either — it anchors `pressureTokens` to the
provider's own reported usage from the most recent request, and only
estimates the *delta* since that anchor (what changed since the last real
measurement) with its own heuristic. Error is bounded to one turn's worth
of drift instead of compounding across a session.

Added the same anchor to `Agent`: a new `record_actual_prompt_tokens`
(`src/agent/compact/main.rs`), called from `run_model_turn_with_tools`
(`src/agent/runtime.rs`) right where `response.usage` is already read into
the running token totals, replaces `compact_state.estimated_history_tokens`
with the provider's own reported prompt size for the request that was just
answered: `usage.input_tokens + usage.cache_hit_tokens + usage.cache_miss_tokens`
(Anthropic wire semantics — `input_tokens` is non-cached tokens only, cache
reads/writes are reported separately and additively, matching
`parse_anthropic_token_usage` in `src/llm/deepseek_anthropic/stream.rs`).
This runs before the turn's own assistant reply and any tool results get
appended to history via `push_history_message`/`extend_history_messages`
further down, so the existing per-message local-estimate accumulation
continues to layer correctly on top of the corrected baseline.

## Scope

This corrects the estimate for every provider that reports `usage`, not
just DeepSeek — Kimi/Moonshot uses its own tokenizer too and was subject to
the same class of drift. Providers that never report usage (some local/
Ollama configurations) keep relying on the local `cl100k_base` estimate
alone, unchanged from before.

Not addressed here (raised during triage, deferred by the user's own
priority call):

- The main turn's own LLM call still has no "catch a context-window
  overflow, force-compact, retry once" recovery path — only compaction's
  own internal summarization sub-call has one
  (`is_context_window_error` in `src/agent/compact/main.rs:327`), and
  that check doesn't even recognize DeepSeek's Anthropic-compatible error
  shape (a plain `anyhow!` string, not the `OpenAiApiError` type it
  downcasts against).
- `compact_if_needed_with_reporter` runs once per user query
  (`query_inner` in `src/agent/runtime.rs`), not before every LLM call
  within a multi-round agentic turn — a single turn that grows history
  substantially through several tool calls has no mid-turn recheck before
  its own next request.

Both remain real gaps; this fix narrows the actual production trigger
(estimate-vs-real drift) without building either recovery path.

## Verification

- `cargo test --lib agent::compact::`: 16 passed, including new
  `total_prompt_tokens_sums_input_and_both_cache_fields` and
  `total_prompt_tokens_handles_no_cache_activity`.
- `cargo test --lib agent::`: 184 passed, 1 ignored, 0 failed — no
  regression in existing compaction/microcompact tests that manually seed
  `compact_state.estimated_history_tokens` before a turn (they assert on
  pre-turn compaction/projection behavior, not the corrected post-turn
  value, so they're unaffected).
- `cargo clippy --lib -- -D warnings` and `cargo fmt --check` (Rust
  sources): clean.
