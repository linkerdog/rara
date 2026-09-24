# DeepSeek Anthropic Route: Context Budget Must Reserve This Route's Own `max_tokens`

## Summary

A real production 400 from `api.deepseek.com/anthropic`, reported after
[2026-09-24-deepseek-anthropic-orphaned-tool-results.md](2026-09-24-deepseek-anthropic-orphaned-tool-results.md)
merged — a different failure mode on the same route (context-window
overflow, not a message-shape rejection):

```
This model's maximum context length is 1048576 tokens. However, you
requested 1051534 tokens (795534 in the messages, 256000 in the
completion). Please reduce the length of the messages or completion.
```

## Root Cause

`DeepseekAnthropicBackend::context_budget` (`src/llm/deepseek_anthropic.rs`)
delegated straight to `self.fallback.context_budget(...)`. `fallback`
(`OpenAiCompatibleBackend`) only widens its heuristic output reservation
when **its own** `max_output_tokens` field is set — the budget for
`chat/completions` requests `fallback` never actually sends on this route.
On the plain configured-provider path
(`build_openai_compatible_backend` in `src/runtime_context.rs`,
`DeepseekAnthropicConfig { ..Default::default() }`), that field stays
`None`, so the compaction threshold fell back to the generic
window-percentage heuristic (`reserved_output_tokens_for_window` in
`src/llm/shared.rs`) — for `deepseek-flash`'s 1,048,576-token window, that
reserves only ~32K tokens for output.

Meanwhile `request_body` always sends the real completion budget to
DeepSeek's Anthropic-compatible endpoint:
`self.max_output_tokens.unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS)`, i.e.
**256,000** tokens by default. History was therefore allowed to grow
toward the ~32K-reserved threshold (~1,007,616 tokens), and once a request
went out at that size the real 256K-token completion reservation overshot
the window — reproduced with a 795,534-token history, well under the
too-permissive threshold but still
`795,534 + 256,000 = 1,051,534 > 1,048,576`.

## Fix

`context_budget` now re-derives `reserved_output_tokens`/
`compact_threshold_tokens` from this route's own `max_tokens`
(`effective_max_output_tokens`: the registry model's `limit.output`, or the
fixed 256k default matching the DeepSeek reference harness), keeping the
same context window and compaction slack `fallback` already computed.

## Follow-Up (Copilot Review)

Two review comments on the first version of this fix:

1. `reserved_output_tokens` in the returned `ContextBudget` was clamped to
   the context window, but `request_body` still sent the **unclamped**
   `effective_max_output_tokens()` on the wire. A registry model can supply
   `limit.output` independent of `limit.context` (accepted when
   `limit.context` is absent), so a misconfigured override larger than the
   window would report a budget suggesting compaction was unnecessary while
   the wire request alone already exceeded the window — 400s on literally
   any history, empty included.

   Fixed two ways: `reserved_output_tokens` is now capped at **half** the
   window rather than the full window — the same safety margin
   `reserved_output_tokens_for_window` already applies to its own
   heuristic — leaving room for input even under a badly misconfigured
   override. And `request_body`'s `max_tokens` now comes from a new
   `wire_max_output_tokens`, which is defined as exactly
   `context_budget(...).reserved_output_tokens` (falling back to the raw
   unclamped value only when no budget is known for the model at all), so
   the wire value and the reported budget can never disagree by
   construction — one is no longer a second, independently-computed copy
   of the other.

2. This PR changes the route's documented context-budget contract
   (previously: `context_budget` delegates entirely to `fallback`) without
   updating `docs/features/deepseek-anthropic-route.md` or adding a journal
   note. Addressed by this entry and the new "Context budgeting" contract
   bullet in the feature doc.

## Verification

- `cargo test --lib llm::deepseek_anthropic::`: all pass, including
  `context_budget_reserves_this_routes_actual_max_output_tokens_not_fallbacks`,
  `context_budget_honors_an_explicitly_configured_max_output_tokens`,
  `context_budget_caps_a_misconfigured_output_override_at_half_the_window`,
  and `request_body_sends_exactly_what_context_budget_reserved_for_output`
  (parameterized over the default, an explicit small override, and a
  larger-than-half-window misconfigured override — asserts `max_tokens` on
  the wire always equals `context_budget`'s `reserved_output_tokens`).
- `cargo clippy --lib -- -D warnings` and `cargo fmt --check`: clean.
