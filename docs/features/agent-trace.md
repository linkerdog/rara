# Agent Trace

## Problem

The runtime exposes a completed-query report and the latest agent-loop state,
but neither is a durable, ordered account of how context selection, model cache
accounting, and agent continuations changed during a session. Inspecting a
single latest view cannot explain a cache-rate regression or a memory-selection
decision across several model requests.

## Scope

- Provide `rara-agent-trace`, a standalone crate for content-free trace records
  and opt-in local JSONL persistence.
- Create one immutable trace directory per runtime session when
  `agent_trace_dir` or `--agent-trace-dir` is configured.
- Record turn start/finish, context selection, model completion, and agent-step
  state transitions.
- Record provider-reported cache hit and miss tokens only when the provider
  supplied usable accounting.
- Reuse the same typed event model for a future OpenTelemetry exporter.

## Non-Goals

- Exporting OTLP, configuring a collector, or depending on an OpenTelemetry
  SDK in this first slice.
- Persisting prompts, responses, memory contents, tool inputs, tool outputs,
  workspace paths, or request fingerprints.
- Changing model requests, transcript persistence, memory selection,
  compaction, cache policy, or runtime-control events.
- Tracing subagent execution before its session-scoped runtime path can pass an
  explicit child trace context.

## Architecture

`rara-agent-trace` owns the schema and file writer. A disabled
`AgentTraceRecorder` is a no-op. An enabled recorder is constructed during
session assembly after the stable session ID is known, then stored on that
session's `Agent`; no process-global trace writer exists.

The configured root receives a unique trace directory containing:

```text
manifest.json
events.jsonl
```

`manifest.json` identifies the schema version and session. `events.jsonl` is
an append-only ordered event stream. Each record has a writer-assigned sequence
number, wall-clock creation time, monotonic elapsed milliseconds, session ID,
optional runtime turn ID, and an event body.

The runtime converts existing structured observations into these event types:

- `turn_started` and `turn_finished` delimit one submitted user turn.
- `context_assembled` captures retrieval candidate/selection/budget counts and
  retrieved-memory budget totals.
- `model_finished` captures one request's model label, duration, finish
  outcome, and optional token/cache receipt.
- `agent_step_updated` snapshots one agent-loop state transition, including
  stop/continuation state and tool-call count.

## Contracts

- Tracing is disabled unless an explicit output directory is configured.
- The writer never records content-bearing fields. The current provider receipt
  has no explicit `cache_accounting_present` flag, so `cache: null` means no
  non-zero cache category was reported; a zero-hit receipt with non-zero misses
  remains explicit as `hit_tokens: 0`.
- Sequence values strictly increase within one trace directory.
- A failed trace write returns an error to the runtime, which logs a warning and
  continues the agent operation unchanged.
- Trace paths are session-scoped. Rebuilding or hosting another runtime creates
  a distinct recorder and never shares mutable writer state.
- Consumers must aggregate cache hit rate from total hit and miss tokens. They
  must not average per-request percentages.

## Validation Matrix

| Case | Validation |
| --- | --- |
| File contract | Crate test creates a trace, reads manifest and JSONL, and checks ordered sequence values. |
| Cache honesty | Crate test distinguishes absent accounting from a verified zero-hit receipt. |
| Runtime integration | Agent test records model, context, and terminal turn events without prompt text. |
| Opt-in behavior | Runtime construction with no trace directory remains a no-op. |
| Formatting | `cargo fmt --check` and `git diff --check`. |

## Operational Notes

`--agent-trace-dir <DIR>` is intended for local diagnostics. The directory may
still reveal session identifiers and timing, so operators should select a
protected local destination and apply their own retention policy.

## Open Risks

- The JSONL schema is intentionally local and versioned; a future OTLP adapter
  must translate from these typed events rather than treat JSONL as OTLP JSON.
- This slice captures only the root runtime. Subagent correlation requires an
  explicit parent/child trace context at the tool-runtime boundary.

## Source Journals

- [2026-09-14-agent-trace](../journal/2026-09-14-agent-trace.md)
