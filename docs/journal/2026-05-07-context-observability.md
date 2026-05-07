# Context Observability View

## Summary

RARA now has a structured `ContextObservabilityView` that summarizes the
runtime context accounting needed by `/context`, ACP/Wire, and future
OpenTelemetry export.

## Runtime Shape

The view is attached to `SharedRuntimeContext` and contains:

- cache usage: cache hit tokens, miss tokens, and hit-rate basis points;
- compaction: estimated history tokens, threshold, count, last before/after
  tokens, and saved tokens;
- microcompact projection: enabled flag, policy budget, kept recent count,
  original/projected/saved chars, cleared result count, and kept result count;
- retrieval: request id, provider/candidate counts, selected/available/dropped
  counts, and budget token totals.
- agent turn trace: execution mode, model stop reason, loop outcome,
  continuation phase, text/reasoning/tool-call booleans, assistant-history
  recording, and consecutive reasoning-only count.

The model is also exposed through a `ContextEvent::ObservabilityUpdated` wire
shape so downstream control planes can consume the same structure without
scraping TUI text.

## Prefix Stability

This change is observability-only. It does not alter prompt source ordering,
tool schemas, stable memory placement, retrieved-memory suffix placement, or
the persisted transcript. Tool-result microcompaction still projects only the
request copy of history and keeps `tool_use` / `tool_result` blocks paired.

## Validation

Focused tests cover:

- runtime assembly of cache, microcompact, and retrieval observability fields;
- structured runtime-control serialization for context observability events;
- agent-level microcompact projection accounting without mutating history.
- reasoning-only continuation traces so "thinking-only then stop" can be
  separated into model output, parser, runtime decision, or follow-up request
  failures.
