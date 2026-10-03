# Terminal Lifecycle Review Follow-Up

## Summary

Follow up the #946, #947, and #948 reviews without changing the established
quit, cancellation, or job-control policy. Fix owner-panic cursor handoff,
remove an unnecessary startup cursor query, and preserve quit confirmation
across passive pointer motion. The owning contracts are RUN-05/RUN-06 and the
composer interrupt/quit rules.

## References And Decisions

Re-inspected Codex `tui/src/tui.rs` panic restoration and viewport ownership at
`ea2046f36d5ee12d39c8e168fc3e5129301afa2b`, and Claude Code
`src/ink/ink.tsx` synchronous exit cleanup at
`4b9d30f7953273e567a18eb819f4eddd45fcc877`. Restoration precedes delegated
diagnostics; for this primary-screen viewport, that also requires a clean
line below the frame. Destructors must not move back over the panic text.
Local Crossterm 0.29.0 source confirms `EnableMouseCapture` enables 1003
any-event tracking, so passive motion is a real input path.

## Review Changes

- Owner-panic restoration attempts mode cleanup and a bottom-row newline before
  calling the previous hook. Cursor/attribute reset errors surface while the
  original cleanup error is preserved. The owner-scoped hook still leaves
  caught worker panics alone.
- `Terminal::new` initializes unreserved geometry without DSR. Inline row
  reservation already uses relative output, so there is no cursor reply to
  wait for or leak into the new input stream.
- Passive mouse movement returns no application event. Click/drag/wheel input
  still disarms quit confirmation. Ignoring motion also avoids the dispatcher's
  general non-quit-event reset.
- Event-stream regressions live in a normal test module with behavioral names;
  the unused test import in `event_loop.rs` is removed.
- Native terminal reflow and stale scrollback fragments remain explicit
  physical-terminal/tmux/macOS acceptance work in TODO. A terminal draw error
  still ends the session; no retry/recovery contract is introduced.

## Preserved Policy And Limits

- Busy Ctrl-C cancels first and confirms exit on a second same-key press within
  one second. Empty-composer Ctrl-D remains quit confirmation. These deliberate
  semantics and tests are retained; terminals without repeat metadata cannot
  distinguish held keys from independent presses.
- After suspend, native termios monitoring protects against late shell writes.
  Failure to open/query the controlling terminal surfaces instead of continuing
  with unverified input modes. SIGTSTP targets the foreground process group,
  including pipeline peers; background resume is outside the contract.
- Clipboard OSC 52 writes remain bounded and serialized on the UI task;
  terminal/native copies may differ while the latest native copy is pending.
  Superseded notices are suppressed, shutdown cancels native work, terminal
  policy controls acceptance, and actual subprocess fixtures are Unix-only.
  The explicit child-reaping follow-up is recorded in the clipboard journal.

## Validation

All three focused guards failed against the unchanged production paths:
startup made one unwanted cursor query, passive motion cleared the armed
shortcut, and the PTY cursor was at row 1/column 4 instead of row 23/column 0
immediately before the previous panic hook.

- `cargo test --locked --lib tui:: -- --nocapture`: 920 passed, three ignored
  subprocess fixtures exercised by their parents.
- `bazel test //:rara_unit_tests --test_arg=tui::`: the same 920 tests passed
  with the default configuration after restoring a missing external rules cache.
- `cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings`:
  passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
