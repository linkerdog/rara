# DeepSeek V4.1 Flash Model Catalog

**Date**: 2026-09-16
**Scope**: Align the DeepSeek OpenAI-compatible provider with the current
DeepSeek model IDs.

## What Changed

- Set `deepseek-flash` as the default DeepSeek model and updated the README
  example.
- Made the fallback picker catalog advertise only `deepseek-flash` and
  `deepseek-v4-pro`, both with a 1M-token context window.
- Retained the retired V4 Flash aliases as manually configurable compatibility
  names with the same context budget, without advertising them in the picker.
- Route inferred auxiliary work for `deepseek-v4-pro` to the canonical
  `deepseek-flash` ID.

## Why

DeepSeek documents `deepseek-flash` as the canonical V4.1 Flash model ID and
lists the older V4 Flash names as temporary compatibility aliases. The catalog
must recommend the current API surface while preserving existing saved
configuration.

## Verification

- Provider catalog tests cover the current IDs, context windows, and fallback
  visibility.
- Focused DeepSeek integration tests cover default selection, context budgets,
  and auxiliary-model routing.
