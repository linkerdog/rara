# 2026-05-13 Observability Crate

## Summary

Added the first `rara-observability` crate as a lightweight process-local
observability boundary. The initial recorder captures bounded memory
read/write/query latency samples and exposes P80/P99 snapshots for `/status`.

## Key Decisions

- Keep the first slice local-only instead of adding a remote OTEL exporter.
- Use fixed-capacity in-memory sample windows.
- Drop samples under lock contention rather than blocking memory operations.
- Keep metrics free of request text, memory content, vectors, prompts, and tool
  payloads.
- Render memory latency under the `/status` metrics tag instead of mixing it
  into resource or context state.

## Validation

- `cargo test -p rara-observability`
- `git diff --check`
- Attempted `cargo test tui::command::tests::status_metrics_text_reports_memory_latency_percentiles -- --nocapture`;
  the build failed before reaching the test because the local disk ran out of
  space while writing `target/debug` artifacts.

## Follow-Ups

- Wire local embedding sidecar cache-hit and backend/model state into the
  `/status` context tag after that sidecar path is present on `main`.
