# Provider Model Catalog And API Fallback

## What changed

The provider catalog now returns typed model entries with an optional context
window instead of bare model names. DeepSeek and Kimi API responses may provide
`context_length`, `context_window`, or `max_context_length`; the catalog uses
that value when present and enriches known models from the static provider
catalog otherwise. Duplicate model IDs are removed while preserving provider
order.

The TUI keeps provider model selection compatible with existing string-based
state while retaining context-window metadata for both provider-specific and
unified model pickers. Model refresh uses one provider-parameterized runtime
maintenance command, and the catalog is retained in `RuntimeSnapshot` so a
snapshot consumer can hydrate the picker without parsing provider responses.
API failures continue to use the static fallback catalog and mark that
projection as fallback data.

## Why

OpenCode separates a durable provider/model catalog from runtime provider
availability and exposes a unified model projection to the TUI. RARA had the
API and fallback paths, but discarded metadata at the catalog boundary, so
models loaded from `/models` could not display or use their context window.

## Trade-offs

This change generalizes the existing DeepSeek/Kimi catalog without introducing
a network request for every provider or moving provider JSON parsing into the
TUI. Other providers continue to use their existing static presets until they
gain a provider-specific catalog adapter.

## Verification

- `cargo fmt --all`
- `cargo test -p rara-provider-catalog --no-fail-fast`
- `cargo check -p rara --locked`
