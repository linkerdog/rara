# Runtime Event Replay And Resynchronization

## Summary

Issue #975 exposed a TUI adapter that logged broadcast lag and discarded the
missing events. Query receipts covered some gaps, but maintenance and background
events could disappear. The adapter now replays an ordered bounded log, reports
unrecoverable ranges, and refreshes current runtime state after retained events
have been applied.

## Reference Review And Plan

Reviewed Codex at `f959e7fc`: `codex-rs/app-server/src/in_process.rs` exposes a
typed lag marker and distinguishes required delivery from lossy notifications;
`codex-rs/tui/src/app/app_server_events.rs` repairs MCP startup state on lag.
Reviewed Claude Code at `4b9d30f7`:
`src/cli/transports/SSETransport.ts` resumes from a sequence cursor and tracks
duplicate frame identities. The existing `rara-runtime::EventLog` already owns
atomic sequence allocation, retention, and publication, so this change reuses
that log rather than creating another ordering domain.

Implementation stages:

1. Capture dropped-prefix behavior in a regression, define the replay contract,
   and add a cancellation-safe adapter over the existing event log.
2. Keep transport cursors separate from session/turn filtering, drain the published
   stream before completion receipts, and report gaps that neither source covers.
3. Refresh authoritative state only after the retained tail and agent return;
   compare lagged/non-lagged projections and validate public session boundaries.

## Key Decisions

- Default compatibility bootstrap uses 256 broadcast slots and 1024 retained
  control events. Explicit session capacity sets both bounds, preserving the
  public session contract. Startup options moved to `runtime_context/options.rs`
  to keep the assembly file below 1000 lines. The old equal-capacity convenience
  constructor is now test-only; production uses named capacity fields.
- Subscription captures a cursor before subscribing and replays from it. Both
  broadcast lag and sequence gaps trigger recovery. Replay/live overlap is
  deduplicated before the controller's existing session and terminal-turn fence.
- A typed recovery gap preserves the retained tail. Query receipts can fill the
  missing prefix silently. Otherwise the transcript warns that some output is
  unavailable; no synthetic tool result or terminal event is produced.
- Query completion first drains the ordered stream through the published cursor
  captured after execution returns, then drains remaining receipts. This prevents
  a later query receipt from hiding an interleaved background event. Recovery
  preserves live records even when Tokio rounds broadcast capacity above the
  exact replay retention count; completion uses the same stream ordering path.
- Gap recovery refreshes goals and agent activity immediately, then waits for
  the retained tail and agent ownership before refreshing the agent snapshot.
  An idle refresh retires stale live progress while retaining transcript text.
  Snapshot synchronization publishes the newly derived state before reading the
  port cache, so an old UI cache cannot undo the authoritative refresh.
- No wire format, persistent schema, public session API, dependency, or Bazel
  configuration changes.

## Validation

The original adapter fails the dropped-prefix regression by returning a retained
event without reporting loss. Focused checks cover:

- bursts larger than broadcast capacity, exact ordering and duplicate rejection;
- subscription races, cancelled receives, and sequence gaps without `Lagged`;
- live records retained beyond the replay window due to broadcast rounding;
- non-query projection equality against an uninterrupted consumer;
- exhausted windows, visible diagnostics, fresh goal/pending-input state,
  deferred agent ownership, and stale live-progress retirement;
- snapshot ordering after retained stale events;
- background events interleaved with query completion and complete receipt
  recovery without a false loss notice.

Validation commands:

```bash
cargo test -p rara --lib tui::
cargo test -p rara --test runtime_session
cargo clippy -p rara --all-targets -- -D warnings
cargo fmt --all
bazel test --test_output=errors //:rara_unit_tests
```

The TUI suite reports 1014 passed and 4 existing ignored tests; the public
runtime-session integration suite reports 8 passed. Strict all-target Clippy and
formatting checks pass.

The default local Bazel check stops during package loading because the existing
external cache lacks `rules_rust//rust`. Remote default Bazel CI remains the
merge gate; no local Bazel settings or dependency locks were changed.

## Follow-Ups

No implementation work is deferred from #975. Replay remains bounded by event
count; an exhausted window cannot reconstruct lost transient output. TUI
migration to the public session actor and broader transcript memory bounds
remain separate tracked work.

## Review Follow-Up

PR #1024 review asks for current-main integration and stronger evidence around
receipt/stream ordering. Main is merged without rewriting the feature branch.
Rechecked the same Codex lag-marker and Claude Code sequence-cursor references.

- Rejected zero-sequence transport records before projection, with a diagnostic.
- Replaced the millisecond quiescence check with an immediate poll. A new
  controller fixture parks later query receipts behind the current stream
  boundary and verifies both intervening background events appear exactly once
  in publication order before query completion.
- Coalesced loss notices while one authoritative refresh remains pending;
  additional gaps extend its cursor target. The regression initially produced
  two notices and now requires one while the agent remains owned by execution.
- The lightweight rendering harness rejects scripted resync markers because it
  lacks query receipts and runtime ownership. Recovery coverage uses production
  controller fixtures, avoiding a synthetic loss notice with different semantics.
- Inspected non-TUI subscribers: hooks consume the raw event channel and still
  drop lag. Memory/protocol control subscriptions found here are test fixtures.
  Hook delivery/replay is tracked in `docs/todo.md`; replaying side effects needs
  a separate idempotency contract and is outside the TUI recovery scope.

The merged-branch TUI suite reports 1028 passed and 4 existing ignored tests.
The original #1024 head passed all 11 remote checks; the new head must pass them
again. `cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings`,
`cargo fmt --all`, and `git diff --check` pass on the merged branch.
