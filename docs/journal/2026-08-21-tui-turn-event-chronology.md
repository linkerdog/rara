# TUI Turn Event Chronology

## What changed

The TUI now uses the active turn entry list as the single chronology owner for
closed assistant and progress segments. A thinking stream is inserted at its
event boundary before assistant text starts, and an open assistant stream is
closed before a later thinking segment starts. Turn commit no longer
materializes a separate live-event sidecar at the end of the turn.

Only the currently open Markdown or thinking stream remains transient. Derived
tool labels stay in the active display cache because the typed tool or terminal
entry already owns that event's transcript position. Explicit thinking,
planning, exploration, and running events enter the ordered transcript and the
active renderer preserves interleaved assistant/progress order while retaining
adjacent progress compaction.

Query completion now waits for both the task result and a terminal runtime
projection. In-process control-plane events are re-sequenced by the runtime bus
across dispatch requests, while legacy raw lifecycle subscribers receive a
raw-only start/stop projection so the ordered control stream has exactly one
terminal boundary per turn.

## Why

Previously, assistant text was written directly to `active_turn.entries`, but
closed thinking and progress were kept in `active_live.events`. Commit appended
that sidecar after all transcript entries, so a real event sequence of
`Thinking -> Agent` became `Agent -> Thinking` in committed history.

The event loop also selected independently between the query `JoinHandle` and
the structured runtime stream. A ready task could therefore commit the turn
before already-published tail events were consumed. Request-local dispatch
sequence numbers reset on each turn, which could additionally make the TUI
reject a later turn's terminal event as stale.

## Reference patterns

The implementation adapts current upstream ordering boundaries:

- Codex `536f86e5cc9ec1ff38457d099bf320b9d08eeeba` finalizes a completed
  reasoning item into history at the reasoning-item boundary instead of
  appending reasoning at turn completion.
- OpenCode `ba72a6ff2b62aaf614b8e745193e86a51be6142c` assigns ascending part
  identities to reasoning, text, and tool parts, closes reasoning on explicit
  lifecycle boundaries, and renders message parts in that stored order.
- The current public Claude Code source surface does not expose an equivalent
  transcript processor. Its older hybrid transport flush-before-terminal
  behavior was used only as corroboration, not as the implementation basis.

## Design decisions

### One ordered closed-segment projection

Closed segments move immediately into `TranscriptEntry`. This avoids wall-clock
sorting, role sorting, and commit-time reconstruction. The display caches still
support compact live summaries, but they no longer own durable ordering.

### Two-signal query completion

Normal query completion requires a task result and a terminal structured event,
regardless of which arrives first. A failed task join is terminal by necessity
because no producer remains to publish a lifecycle event. A closed runtime
stream is projected as a disconnect so the TUI fails visibly instead of
waiting indefinitely.

### Bus-owned local sequence

External adapter events keep their original event identity and sequence. Only
in-process dispatch events enter the bus-owned monotonic sequence domain. This
keeps cross-turn TUI ordering stable without changing ACP or Wire provenance
contracts.

## Verification

- `cargo fmt`
- `cargo check --all-targets`
- `cargo test --all-targets --quiet` (`1321` library tests and the embedded
  integration test passed)
- `cargo clippy --all-targets -- -D warnings`
- focused active-turn, committed-turn, controller-barrier, runtime-event-bus,
  and runtime-event projection tests
- `git diff --check`

The local machine has no CUDA compiler, so an exploratory
`cargo clippy --all-targets --all-features -- -D warnings` stops in the
`cudarc`/Candle build scripts before checking RARA. The default feature gate
above is warning-clean.

## Remaining work

No follow-up is required for this chronology correction. Interactive expansion
and richer elapsed-time display remain the independent thinking-display items
already documented in the feature spec.
