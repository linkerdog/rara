# Kimi K3 Support

## Summary

Added Kimi K3 to RARA's Kimi provider catalog support while keeping the existing
Kimi default model unchanged.

## What Changed

- Added `kimi-k3` to the Kimi fallback model catalog.
- Recorded the `kimi-k3` context window as 1,048,576 tokens so budgeting does
  not depend on a live model-list request.
- Updated the default Kimi API endpoint to the current official
  `https://api.moonshot.ai/v1` service address.
- Kept `kimi-k2.6` as the default Kimi model to avoid changing existing cost and
  behavior expectations.

## Verification

- `cargo test -p rara-provider-catalog kimi::tests::fallback_models_include_kimi_k3_first`
- `cargo test llm::tests::derives_context_budget_for_kimi_k3`
