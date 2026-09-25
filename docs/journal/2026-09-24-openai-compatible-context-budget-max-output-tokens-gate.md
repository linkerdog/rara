# OpenAiCompatibleBackend: Context Budget Must Reserve `max_output_tokens` Whenever It's Set

## Summary

Same class of bug fixed for the DeepSeek Anthropic route in #905/#907
(the compaction budget's `reserved_output_tokens` diverging from the
`max_tokens` a request actually puts on the wire), found by auditing every
`LlmBackend::context_budget` implementation after the question "is this
specific to the DeepSeek Anthropic wrapper, or shared?" It is shared — the
same shape of bug exists in the base `OpenAiCompatibleBackend` used by
every `chat/completions`-style provider.

## Root Cause

`OpenAiCompatibleBackend::context_budget` (`src/llm/openai_compatible.rs`)
only widened `reserved_output_tokens` to match `self.max_output_tokens`
when `self.configured_api_root` was *also* set:

```rust
if self.configured_api_root.is_some()
    && let Some(output) = self.max_output_tokens
{ /* widen reserved_output_tokens to output */ }
```

`configured_api_root` is set only by `with_provider_model` — the
registry-model construction path, which sets `configured_api_root` and
`max_output_tokens` together atomically from `model.limit.output`.

But `chat_completion_request_body` sends `self.max_output_tokens` on the
wire **unconditionally**, with no such guard:

```rust
if let Some(max_output_tokens) = self.max_output_tokens {
    body["max_tokens"] = json!(max_output_tokens.get());
}
```

A caller that sets `max_output_tokens` through the public
`with_max_output_tokens` alone — without going through `with_provider_model`
— had that value sent on every request while `context_budget`'s override
sat dormant, leaving `reserved_output_tokens` at the generic
window-percentage heuristic. Same "wire value and compaction budget
independently drift apart" shape as the DeepSeek Anthropic bug.

## Exposure

Currently reachable only through the two callers of `with_max_output_tokens`:

- `src/deepseek_cache_probe.rs:311` — a diagnostic cache-probe tool.
- `src/agent/tests/cache_trial/driver.rs:367` — a cache-trial test harness.

Both are opt-in measurement/diagnostic tooling, not the normal
`chat/completions` path (which always goes through `with_provider_model`,
where the guard already held correctly) — no user-facing regression from
this bug in the normal chat flow. But it's a live landmine for those
tools, which specifically exist to push large/custom token budgets to
test caching behavior — exactly the scenario most likely to trip this.

## Fix

Dropped the `configured_api_root` requirement: the override now applies
whenever `max_output_tokens` is set, matching the wire-send condition
exactly.

## Out of Scope

- `OpenAiCompatibleBackend` also sends `chat_completion_request_body` for
  an auxiliary/summary model that can differ from `self.model`
  (`summarize_with_model`); `max_output_tokens` isn't per-model. Not
  addressed here — `chat_completion_request_body`'s `model` parameter only
  sets the wire `model` field, and `max_output_tokens` is a single field
  regardless, so this predates and is orthogonal to this fix.
- Not applying the DeepSeek Anthropic route's "fail loud on an impossible
  budget" hardening (`ensure_output_budget_fits_window`, #907) here: this
  backend is far more widely shared (every `chat/completions`-style
  provider) with more request call sites (`ask_with_context`,
  `ask_streaming_with_context`, `request_cache_fingerprint`,
  `summarize_with_model` for both the main and auxiliary model), so it
  deserves its own separate consideration rather than a reflexive copy of
  that pattern.

## Verification

- `cargo test --lib llm::`: 175 passed, including a new
  `context_budget_reserves_max_output_tokens_even_without_a_configured_provider_model`
  regression test (constructs a backend via `with_max_output_tokens` alone
  and asserts `context_budget` reserves that value instead of the ~32K
  heuristic).
- `cargo clippy --lib -- -D warnings` and `cargo fmt --check` (Rust
  sources): clean.
