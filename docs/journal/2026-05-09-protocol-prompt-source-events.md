# Protocol Prompt Source Lifecycle Events

## Context

Live protocol prompt sources now flow from the shared registry into prompt
runtime at the user-query boundary. External subscribers still needed a
structured event that distinguishes registration from actual prompt injection.

## Changes

- Added `PromptSourceEvent::Injected`.
- `PromptSourceRegistry::list_prompt_sources_for_query()` now emits `Injected`
  for every source snapshotted into the query.
- Turn-limited expiration still emits `Dropped` after the atomic query snapshot
  removes expired sources from the registry.
- Added focused registry coverage for `Registered` → `Injected` → `Dropped`
  lifecycle events.

## Remaining Work

- Extend the same lifecycle-event pattern to protocol skill and hook visibility.
- Surface protocol prompt-source lifecycle events in ACP/Wire subscribers once
  those adapters expose typed prompt-source controls.
