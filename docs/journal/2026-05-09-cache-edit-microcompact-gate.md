# Cache-Edit Microcompact Gate

## Context

Tool-result microcompact already projects older high-volume tool results out of
the per-request model input while preserving the durable transcript. The open
gap was making future provider cache-edit behavior explicit and provider-gated
instead of implied by endpoint shape.

## Change

- `ToolResultProjectionPolicy` now carries `cache_edit_eligible`.
- `Agent` derives that flag from `LlmBackend::cache_profile().cache_edit` for
  both request projection and runtime context assembly.
- `ToolResultProjectionReport` and `MicrocompactProjectionContextView` now
  expose `cache_edit_eligible` and `cache_edit_applied`.
- No backend applies cache edits yet. Current providers either report no
  cache-edit capability or continue using ordinary request projection.

## Invariant

Cache-edit microcompact must never be inferred from an OpenAI-compatible API
shape. A backend must explicitly declare cache-edit support through its cache
profile before the runtime marks a request eligible.

## Validation

- Added a focused projection test that records eligibility without applying a
  cache edit.
- The default path remains disabled for providers that do not declare support.
