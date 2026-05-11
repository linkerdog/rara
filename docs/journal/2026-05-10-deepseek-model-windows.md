# 2026-05-10 — DeepSeek model windows map

## What was done

- Added `MODEL_WINDOWS` map in `crates/provider-catalog/src/deepseek.rs`:
  key = model name, value = context window tokens.
  Entries: deepseek-chat (64K), deepseek-reasoner (64K),
  deepseek-v4-flash (1M), deepseek-v4-pro (1M).

- Updated `FALLBACK_MODELS` to include deepseek-v4-* variants.

- Replaced `DEEPSEEK_LONG_CONTEXT_WINDOW_TOKENS`,
  `DEEPSEEK_LONG_CONTEXT_MODEL_MARKERS`, and `is_deepseek_long_context_model`
  in `src/llm/shared.rs` with a direct lookup against the provider catalog map.
  New models only need an entry in the map, not code changes.

## What remains

- OpenTelemetry models (`UNIFIED_MODEL_PRESETS`) should also move to
  the catalog pattern.
- Model list picker should display context window sizes.
