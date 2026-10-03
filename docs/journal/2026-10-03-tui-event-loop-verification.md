# Production TUI Event Loop Verification

## Motivation And References

The #938 review correctly identified that controller/scheduler tests copied parts
of the loop without exercising its actual select branches and dirty-state wiring.
This completes the production-loop portion of #927 after #949's lint/oracle gates.
The owning contract is QUALITY-06 in the interaction quality specification.

Inspected Codex `ea2046f36d5ee12d39c8e168fc3e5129301afa2b`,
`tui/src/tui/event_stream.rs` and `tui/src/tui/frame_requester.rs`: injectable
input ownership and paused-time scheduling tests separate terminal reads from
frame requests. Inspected Claude Code `4b9d30f7953273e567a18eb819f4eddd45fcc877`,
`src/ink/ink.tsx`: injected stdin/stdout, throttled rendering, and resize-driven
repainting. Adapt these boundaries to the existing controller and scheduler;
do not introduce another event loop or a production scheduler replacement.

## Plan And Scope

1. Extract a private input/mode adapter and accept the existing generic terminal
   backend. Preserve reader release before shell handoff and suspend. Exit when
   the live loop can run against the terminal emulator without a real TTY.
2. Drive the same loop with scripted input/runtime events and paused Tokio time.
   Observe synchronized ANSI frame completions and vt100 screen contents. Exit
   when minimal production mutations fail at the intended behavioral assertions.
3. Validate the final code, including existing PTY lifecycle guards, and publish
   the parent-relative change. Keep physical terminal acceptance in #925.

Only local code/docs, generated dependency metadata, existing test targets, and
normal branch/PR writes are needed. No protocol, persistence, public extension
API, or Bazel configuration/BUILD changes are planned.

## Implementation

- `EventSource` owns input, maintenance, and suspend callbacks. The live adapter
  wraps Crossterm and the session mode guard; the test adapter owns a channel and
  records maintenance/suspend calls. It never signals the test process group.
- `run_event_loop` remains the sole select loop. Controller, command processor,
  frame scheduler, translation, renderer, and dirty-state transitions are shared.
- Frame dimensions come from the drawing backend, avoiding a separate global
  size query in tests and retaining the stdout terminal query in production.
- Replaced the old hand-written controller/scheduler imitation with production-loop
  checks, including runtime command application and actual task-join completion.
- Tests count real synchronized-update terminators and inspect vt100 output.
  Tokio's dev-only `test-util` feature permits explicit clock advancement;
  frame/maintenance timing assertions do not depend on wall-clock sleeps. The
  clock advances past the millisecond timer rounding boundary for due frames,
  while a separate 16 ms assertion retains the no-early-frame check.

## Validation

Five independent minimal production mutations produced behavioral failures:

| Mutation | Guard and observed failure |
| --- | --- |
| Discard runtime projection in the select branch | The due frame lacks the ordered streaming text; no final full-text event can mask missing deltas |
| Remove the frame-wakeup select branch | Ordered burst test: the trailing frame stays at 1 instead of 2 after its deadline, without new input or maintenance |
| Read dirty state without consuming it | Idle maintenance test: frame count becomes 2 instead of remaining 1 |
| Omit resize invalidation | A shrink/grow burst ending at the original size leaves the injected `GHOST` cells visible |
| Discard the maintenance branch's changed flag | Expired quit confirmation: frame count stays at 1 instead of 2 |

The ordered burst guard checks both the streamed screen and the accumulator
before any full-text replacement event. Each mutation was applied alone to
`event_loop.rs`, compiled, and failed at the intended test assertion. The script restored the original production source
before final validation. Compilation failures and fixture corrections are not
counted as RED evidence. The initial permission-command assertion incorrectly
expected a notice hidden by Planning status; the corrected oracle checks the
actual read-only planning display and the typed permission state. Strengthening
the stream-order oracle also required seeding the user transcript: the fake port
records input commands without synthesizing their runtime transcript. Without
that valid turn precondition, the renderer correctly selected its empty-session
status path. This fixture failure is not counted as regression evidence.

- Focused loop suite: nine tests passed with the final turn fixture.
- `cargo test --locked --lib tui:: -- --nocapture`: 928 passed, three ignored
  isolated subprocess fixtures exercised by their parent tests.
- `bazel test //:rara_unit_tests --test_arg=tui::`: the same 928 tests passed;
  the final run needed no retry.
- `cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings`:
  passed on the final source.
- Formatting and whitespace checks passed; all touched Rust files remain below
  1000 lines. Existing snapshots are unchanged.

Default dependency resolution hit missing external cache files; a scoped
force fetch for the default test target restored them without changing Bazel
configuration. Generated lockfile changes are limited to the crate extension's
recorded manifest input hash. Bazel retains the pre-existing gold-linker
warning; this change adds no compiler or Clippy warning. Remote CI is separate
from these local results.

## Limits

Paused Tokio time does not advance `std::time::Instant`. Quit-expiry tests seed
an already-expired timestamp and test the loop's maintenance-to-frame wiring.
Suspend callbacks test routing only; isolated PTY children continue to prove OS
ownership, mode restoration, and process-group behavior. Terminal emulation does
not establish native scrollback reflow or physical macOS/tmux acceptance.
