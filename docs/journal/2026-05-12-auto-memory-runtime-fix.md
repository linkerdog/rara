# 2026-05-12-auto-memory-runtime-fix

## Summary

Fixed the TUI auto-memory trigger so it no longer panics inside the Tokio
runtime, no longer silently drops normal `You` / `Agent` transcript turns, and
now follows a process-local service model closer to Claude Code's
same-model background extraction path.

## Background

The initial auto-memory rollout scheduled extraction after every five committed
turns, but it coordinated trigger state with `tokio::sync::Mutex::blocking_lock`
from the async TUI completion path. When the fifth turn finished inside the
runtime thread, Tokio panicked with:

`Cannot block the current thread from within a runtime`

The same path also filtered transcript roles as `user` / `assistant`, while the
TUI stores live turns as `You` / `Agent`, which meant the extraction payload
could be empty even when the panic did not fire.

## Scope

- `src/auto_memory.rs`
- focused regression coverage in the same module

## Key Decisions

- Replace the blocking mutex gate with a process-local `AutoMemoryService`
  managed through a short-lived synchronous state lock, so turn completion only
  stages work and never blocks the Tokio runtime thread.
- Keep the trigger entrypoint synchronous and lightweight. The async work still
  happens in `tokio::spawn`, but it now runs through one single-flight worker
  loop instead of per-boundary fire-and-forget scheduling.
- Normalize transcript roles from both TUI (`You` / `Agent`) and model-facing
  (`user` / `assistant`) forms before building extraction messages.
- Preserve chronological order and coalesce safely by carrying transcript turns
  since the last successful boundary, then slicing them at execution time so a
  skipped intermediate boundary does not drop turns 6-10 when 15 supersedes 10.
- Align the durable contract with Claude Code's memory forks: auto-memory uses
  the same active agent backend/model route rather than a separate auxiliary
  model.
- Upgrade the scheduler from a stateless once-per-boundary helper to a
  process-local service that allows only one in-flight extraction, coalesces
  newer eligible snapshots into one trailing run, and ignores duplicate
  notifications for the same completed-turn boundary.
- Add a bounded shutdown drain hook so TUI exit gives in-flight auto-memory a
  short chance to finish without letting quit block indefinitely.

## Validation

```bash
cargo test auto_memory::tests -- --nocapture --test-threads=1
cargo check
```

## Follow-Ups

- Add auto-memory observability and controls from `docs/todo.md`
  (enable/disable, last-run status, error reporting, dedupe metrics, stale or
  timeout diagnostics).
- Decide whether auto-memory should eventually use a persistent cross-session
  watermark model similar to the larger Codex memory pipeline, or remain a
  best-effort live-transcript promotion path.
