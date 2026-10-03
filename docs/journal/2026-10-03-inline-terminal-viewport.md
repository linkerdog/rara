# Inline Terminal Viewport And Shell Handoff

## Summary

The terminal now owns one primary-screen viewport containing both transcript
and composer. Startup reserves rows below the shell cursor instead of erasing
shell output. Synchronized frames cover reservation, resize reconciliation,
painting, and cursor placement. Normal exit hands the shell a clean line below
the last frame. Focus reporting updates the displayed terminal state.

This is the viewport and focus portion of issue #925. Keyboard exit policy and
Unix suspend/resume remain separate work.

## Reference Review And Plan

The reference review used Codex `ea2046f36d5ee12d39c8e168fc3e5129301afa2b`
(`tui/src/tui.rs`, `custom_terminal.rs`, and `tui/job_control.rs`) and Claude
Code `4b9d30f7953273e567a18eb819f4eddd45fcc877`
(`src/hooks/useExitOnCtrlCD.ts`). Codex groups viewport changes and drawing in
synchronized output and restores terminal modes before job control. Claude
Code gives an interrupt consumer priority over the exit confirmation gesture.

The selected first step fixes terminal ownership and proves the emitted bytes
before changing keyboard exit or suspension behavior. The runtime, cancellation
barriers, and existing terminal-mode guard remain the owning boundaries.

## Decisions

- Remove the outer composer-height subtraction. The renderer already allocates
  the bottom pane inside the frame; subtracting twice left unused rows and made
  ordinary composer growth change the outer viewport.
- Reserve new rows relative to the actual shell cursor. This also preserves
  history when the optional initial cursor query fails. Do not use ED2 or ED3.
- Keep primary-screen transcript navigation in the existing application model.
  Native scrollback preserves pre-session shell output; this change does not
  add per-turn native-history insertion or an alternate-screen mode.
- Invalidate all cells after resize, including blank first columns. A private
  marker in the previous diff buffer forces repaint and is never emitted. An
  explicit resize event also invalidates when coalesced sizes end at the old
  dimensions.
- Separate the event loop from its terminal handoff so recoverable loop errors
  can position the shell cursor before mode restoration. Keep the first error
  and log cleanup failures. Panic unwinding retains the existing guard behavior
  and avoids moving back over diagnostics emitted by the panic hook.
- Enable focus reporting with the other modes. Cleanup attempts focus and
  synchronized-output restoration along with every existing owned mode, even
  after another cleanup operation fails.
- Add a test-only vt100 backend that consumes production ANSI output while
  providing deterministic dimensions and cursor replies. Existing renderer
  harness tests continue to cover state and layout independently.

## Validation

The initial regressions failed on the previous behavior: a 24-row terminal
received a 19-row outer viewport, and `FocusLost` left focus status true.

Focused terminal-emulator checks cover shell history, unavailable cursor
replies, resize through narrow dimensions, blank-cell repaint, a resize burst
ending at the original dimensions, exit after an unpainted resize, idempotent
shell handoff, re-reservation,
and synchronized-output closure after a write failure. Existing PTY restoration
scenarios also verify focus mode cleanup and normal focus mode enablement.

Commands:

- `cargo fmt --all`
- `cargo test --offline --locked -p rara --lib tui::`
  — 887 passed; the existing PTY subprocess fixture remains intentionally ignored
  in the parent runner and is exercised by its parent test.
- `cargo clippy --offline --locked --all-targets --no-deps -- -D warnings`
  — passed.
- `bazel test //:rara_unit_tests --test_arg=tui::` — passed with the default
  configuration. A target-scoped `bazel fetch --force //:rara_unit_tests`
  restored incomplete external-repository caches and generated the vt100/vte
  dependency metadata. An earlier `bazel mod deps` failed while loading
  unrelated missing extension packages. No configuration or BUILD files were
  edited to work around those cache failures.

## Follow-Ups

Complete Ctrl-C routing and Unix Ctrl-Z suspend/resume under issue #925, then
perform bounded acceptance in a real terminal and tmux. The vt100 tests cover
emitted bytes; they do not establish every emulator's native resize/reflow
behavior. The broader interaction-harness work in #927 remains open.
