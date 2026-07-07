# Exec Startup Panic JSONL

## Summary

Added a headless `rara exec --json` panic hook for initialization failures that
occur before the normal exec event processor starts.

## Background

A Harbor Terminal-Bench smoke run reached `rara-exec.status=101` with an empty
`rara-exec.jsonl`. That means RARA panicked before emitting `thread.started`,
so the benchmark adapter could only report a missing task artifact after
verification.

## Changes

- `rara exec --json` now installs a narrow panic hook before cwd switching and
  runtime bootstrap.
- Startup panics emit `thread.started`, `turn.started`, and `turn.failed`
  JSONL events with the harness `run_id` and `task_id`.
- The hook is gated by a per-run startup-complete flag, so panics after the
  normal exec event processor starts do not inject duplicate startup events.
- The hook still calls the default panic hook, preserving stderr and backtrace
  output for `/logs/agent/rara-exec.stderr`.

## Validation

- `cargo test exec_consumer::tests::startup_failure_events_preserve_harness_metadata`
- `cargo fmt`
