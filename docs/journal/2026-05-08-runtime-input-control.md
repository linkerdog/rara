# Runtime Input Control Checkpoint

This checkpoint starts the P0+ input-control bridge described in
`runtime-control-plane.md`.

## What Changed

- Added a local semantic input-control helper for the TUI runtime path.
- Routed composer submit, busy follow-up queueing, pending request-input
  answers, plan approvals, shell approvals, and cancel requests through that
  helper.
- Published structured input/session events when semantic input actions are
  applied.
- Kept raw terminal keys such as `Esc` inside local key mapping. Busy `Esc`
  becomes a cancellation intent; overlay close and editor navigation remain UI
  state.
- Documented the preemption rule for blocked protocol input: adapters use
  `InterruptCurrentTurn` or `CancelCurrentTurn`, and the runtime decides whether
  the active turn can actually pause or stop.
- Added follow-up planning for a `support-acp` integration skill so IDE and
  third-party app authors can connect through ACP/control-plane contracts
  without copying TUI internals.

## Boundary

This is not the final appserver bridge. `control_plane::dispatch` still lacks a
runtime handle that can mutate the active TUI/runtime state. The next slice
should move this semantic handler behind a protocol-neutral runtime handle and
route `RuntimeControlRequest::Input` plus `SessionControlRequest::CancelCurrentTurn`
through it.

## Validation

No local tests were run for this checkpoint. CI should cover the focused input
control tests and existing TUI submit behavior.
