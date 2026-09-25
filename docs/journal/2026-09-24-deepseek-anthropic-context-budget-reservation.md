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

   `request_body`'s `max_tokens` now comes from a new
   `wire_max_output_tokens`, which is defined as exactly
   `context_budget(...).reserved_output_tokens` (falling back to the raw
   unclamped value only when no budget is known for the model at all), so
   the wire value and the reported budget can never disagree by
   construction — one is no longer a second, independently-computed copy
   of the other. (An intermediate version of this fix also capped
   `reserved_output_tokens` at half the window as a silent normalization;
   see the next section for why that was replaced.)

2. This PR changes the route's documented context-budget contract
   (previously: `context_budget` delegates entirely to `fallback`) without
   updating `docs/features/deepseek-anthropic-route.md` or adding a journal
   note. Addressed by this entry and the new "Context budgeting"/
   "Misconfigured output cap" contract bullets in the feature doc.

## Follow-Up (Fail Loud, Not Silent Clamp)

Comparing against DeepSeek's own reference harness (`deepseek-ai/deepseek-harness`)
surfaced a better answer to review comment 1 above than clamping.
`resolveCompactSpec` (`packages/compaction/compaction-basic/src/config.ts`)
takes the routed request's actual reserved completion tokens as an
explicit input (`reservedCompletionTokens` in
`packages/compaction/compaction-basic/src/index.ts`, read straight from
the request's own `config.maxTokens`) and throws a
`TargetPressureConfigError` the moment that reservation alone — plus its
own `headroomTokens` slack — would leave no message budget in the
context window, rather than normalizing the value down and letting the
turn proceed on a budget nobody configured.

Replaced the half-window clamp with the same fail-loud behavior:

- `context_budget` now returns `None` — "no usable budget" — whenever
  `effective_max_output_tokens()` alone would drive
  `compact_threshold_tokens` to `0` (the reservation plus compaction's own
  slack margin consumes the whole window), instead of silently capping
  `reserved_output_tokens` at half the window.
- A new `ensure_output_budget_fits_window`, called at the top of
  `ask_streaming_once` (before `request_body` is even built), turns that
  `None` into a hard `anyhow` error naming the misconfigured
  `max_output_tokens` and the model's actual context window — so a
  misconfigured registry model fails the turn immediately and legibly,
  the same way DeepSeek's own harness fails config resolution, instead of
  quietly sending a reduced completion budget.

## Follow-Up (Copilot Review on the Fail-Loud Change)

Two more review comments, on the fail-loud version:

1. `context_budget`'s `None` is also how `Agent::compact_history_with_reporter`
   (`src/agent/compact/main.rs`) represents an *unknown* budget — when no
   budget is available at all, it falls back to a generic 10,000-token
   compaction threshold rather than skip compaction outright. That default
   is low enough that ordinary history routinely crosses it, so a
   misconfigured backend's `None` could trigger a summarize call — which
   calls `fallback.summarize`/`summarize_with_context` directly, never
   through `ask_streaming_once` — before any turn ever reached this route's
   own guard. `fallback`'s `chat_completion_request_body` sends
   `max_output_tokens` unconditionally regardless of which model string is
   passed (see #906), so that summarize call would still put the oversized
   value on the wire.

   Fixed by calling `ensure_output_budget_fits_window` from every method
   that can reach the wire with `self.max_output_tokens` — `summarize`,
   `summarize_with_context`, `classify_with_context`, and
   `summarize_with_prefix`, alongside `ask_streaming_once` — instead of
   only the one this fix originally added it to. This makes the
   `None`-ambiguity in the compaction driver harmless in practice:
   compaction may still attempt on a misconfigured backend using the
   generic 10K threshold, but it can no longer succeed in sending a broken
   oversized request — it now hits this route's own clear configuration
   error on every path, not just the direct-ask one.

2. The feature doc's "Open Risks" section still said a misconfigured
   `limit.output` was "clamped to half the window ... rather than rejected
   outright" — stale text left over from the clamp this fix replaced,
   directly contradicting the new "Misconfigured output cap" contract
   bullet. Corrected.

## Verification

- `cargo test --lib llm::deepseek_anthropic::`: 26 passed, including
  `context_budget_reserves_this_routes_actual_max_output_tokens_not_fallbacks`,
  `context_budget_honors_an_explicitly_configured_max_output_tokens`,
  `context_budget_returns_none_when_output_override_leaves_no_room`,
  `ensure_output_budget_fits_window_rejects_a_misconfigured_output_override`,
  `ensure_output_budget_fits_window_accepts_well_configured_backends`,
  `request_body_sends_exactly_what_context_budget_reserved_for_output`
  (asserts `max_tokens` on the wire always equals `context_budget`'s
  `reserved_output_tokens` for every configuration `context_budget`
  actually accepts), and
  `summarize_paths_reject_a_misconfigured_output_override_before_reaching_fallback`
  (proves `summarize`/`summarize_with_context`/`classify_with_context`/
  `summarize_with_prefix` all reject before ever calling `fallback`).
- `cargo clippy --lib -- -D warnings` and `cargo fmt --check` (Rust
  sources): clean.
