# 2026-09-14 Agent Trace

## Summary

Added an opt-in, session-scoped agent trace recorder in a dedicated crate. It
writes a content-free manifest and ordered JSONL event stream for local
diagnosis of context selection, provider cache receipts, model latency, and
agent-loop outcomes.

## Key Decisions

- Keep the trace model local and typed; OpenTelemetry is a future export adapter
  rather than the recorder's persistence format.
- Preserve the distinction between missing cache accounting and verified zero
  cache tokens.
- Store no prompt, response, memory, tool, path, or request-fingerprint data.
- Keep recorder ownership inside one runtime session and surface write failures
  as warnings without changing agent behavior.

## Validation

- `cargo test -p rara-agent-trace`
- `cargo test agent::tests::agent_trace -- --nocapture`
- `cargo check -p rara`
- `cargo fmt --check`
- `git diff --check`

## Follow-Ups

- Add an OTLP exporter that maps the typed local event model to the evolving
  OpenTelemetry GenAI semantic conventions.
- Propagate explicit trace context through the subagent runtime boundary.
