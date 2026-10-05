# Transcript Selection Release Recovery

## Background

Issue #979 reports that an overlay or a dropped mouse release can strand the
transcript in drag mode. Wheel input is then discarded and edge autoscroll
continues. The previous implementation guarded mouse routing by overlay state
but did not cancel selection when ownership changed.

## Reference Inspection And Plan

- Codex at `f959e7fc9832dfa0ebfb6542ab1bbf829638ac24`,
  `codex-rs/tui/src/tui/event_stream.rs`: focus state is updated at terminal
  event translation. Mouse selection remains terminal-owned, so there is no
  application drag lifecycle to port.
- Claude Code at `4b9d30f7953273e567a18eb819f4eddd45fcc877`,
  `src/ink/components/App.tsx`, `src/ink/selection.ts`, and
  `src/hooks/useCopyOnSelect.ts`: focus loss, no-button motion, and a fresh
  press recover lost releases; selection updates require an active drag.
  Finishing can trigger copy-on-select, whereas clearing cancels the gesture.
- Adapt the observable-event recovery pattern to the existing selection owner.
  Clear at overlay/focus/suspend boundaries, recover on motion and wheel, and
  replace stale state before validating a new press. Ignore orphaned Drag/Up.
  Keep ordinary left-release copy and stationary edge autoscroll unchanged.
- Verify transitions through terminal event translation, dispatch, and rendered
  transcript geometry. Assert immediate cancellation before another frame,
  no clipboard initialization, and successful wheel routing after recovery.

## Trade-offs

Cancellation does not copy an uncertain selection. There is no inactivity
watchdog because terminals may send no events while the user holds a stationary
button for edge autoscroll. Recovery occurs on the next observable cancellation
signal, including wheel input on terminals that omit focus/motion reporting.

## Implementation

- `open_overlay` cancels selection before changing overlay ownership. Terminal
  translation clears it on focus loss and before suspension. Passive no-button
  motion requests a redraw only when it cancels an active drag.
- Vertical wheel input clears the drag before routing the same event to its
  current owner. Selection start clears old endpoints before checking the new
  position, so a click in the composer also recovers a dropped release.
- Drag and release translation require an accepted press. Late events after
  cancellation cannot resume selection or initialize clipboard work.
- The existing selection snapshot, wrapping, copy-on-release, wheel
  acceleration, and edge-autoscroll calculations are preserved.

## Validation

All seven new harness regressions fail against the previous implementation at
their intended cancellation/routing assertions. Fixtures use production-rendered
geometry and terminal dispatch without sleeps or real clipboard writes.

- `cargo test --lib tui::selection_input_tests -- --nocapture`: seven passed.
- `cargo test --locked --lib tui:: -- --nocapture`: 1,052 passed, four existing
  ignored fixtures. This includes ordinary copy dispatch, selection wrapping,
  edge autoscroll, scroll bounds, and passive-motion quit confirmation.

- `cargo clippy --locked --all-targets --no-deps -- -D warnings`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.

Physical terminal event delivery is not claimed by the harness; the recovery
contract only uses observed events.
