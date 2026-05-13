# Observability

## Problem

RARA has several user-visible runtime surfaces, but small runtime metrics are
still scattered across feature code. `/status` should be able to show
lightweight operational facts without making memory, model, or context paths
depend on heavyweight telemetry exporters.

The first concrete need is memory latency visibility. Users should be able to
see whether memory read, write, and query paths are slow while keeping the
measurement path cheap enough that observability cannot become the source of
latency or memory growth.

## Scope

- Add a dedicated `rara-observability` crate for process-local runtime
  observability primitives.
- Provide bounded in-memory latency snapshots for memory read, write, and query
  operations.
- Show memory latency P80 and P99 in the `/status` metrics tag.
- Keep `/status` context and metrics distinct:
  - `context` explains current runtime state and selected context providers.
  - `metrics` explains bounded runtime measurements.

## Non-Goals

- Exporting OTEL, StatsD, Prometheus, or remote analytics in this slice.
- Persisting metrics across process restarts.
- Recording request text, memory content, vectors, prompts, or tool payloads.
- Building a full tracing hierarchy for memory operations.
- Measuring local embedding sidecar cache-hit state before the sidecar path is
  present on `main`.

## Architecture

The crate follows the useful shape of Codex's telemetry split: instrumentation
is isolated in a small crate and product code records through a narrow API.
Unlike Codex's full OTEL crate, this first RARA slice is intentionally local and
bounded.

`rara-observability` owns:

- `MemoryOperation`: `read`, `write`, and `query`.
- `MemoryObservability`: a process-local recorder.
- `MemoryLatencySnapshot`: a copyable status snapshot with per-operation P80,
  P99, and sample count.
- A global process-local memory observability handle for ordinary runtime code.

Memory samples are stored in fixed-capacity windows. Recording is best-effort:
if the recorder lock is contended, the sample is dropped instead of blocking the
memory path. Snapshot reads are also best-effort and return an empty snapshot if
the recorder is currently busy.

`/status` consumes only snapshots. It must not inspect memory records, request
payloads, vectors, or LanceDB internals while rendering metrics.

## Contracts

- Observability recording must not block memory read, write, or query paths.
- Samples are bounded per operation and do not grow with session length.
- Metrics are process-local and non-persistent.
- Metrics store durations only. They must not store request text, memory
  content, vectors, prompts, or tool outputs.
- Percentile computation may allocate only when `/status` snapshots are read;
  ordinary memory operations must not allocate beyond fixed recorder windows.
- `/status` metrics should render empty values as `-`.
- `/status` context remains the place for runtime state such as backend/model,
  cache-hit state, and setup/reuse detail.

## Validation Matrix

| Case | Validation |
|---|---|
| Bounded windows | Unit test records more samples than capacity and verifies only the latest window is summarized |
| Percentiles | Unit test verifies P80/P99 nearest-rank results |
| Timer recording | Unit test verifies a dropped timer records one sample |
| `/status` metrics | TUI status test verifies memory read/write/query latency fields render |
| Whitespace/docs | `git diff --check` |

## Open Risks

- The first implementation is process-local only, so multi-process RARA sessions
  do not share metrics.
- P80/P99 are approximate over a small recent window, not durable service-level
  objectives.
- Local embedding sidecar cache-hit context still needs to be wired after the
  sidecar lands on `main`.

## Source Journals

- [2026-05-13-observability-crate](../journal/2026-05-13-observability-crate.md)
