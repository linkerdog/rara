# TUI Terminal Restoration

## Summary

Issue [#916](https://github.com/linkerdog/rara/issues/916) exposed terminal mode
ownership outside the error boundary of the UI. RUN-05 now requires restoration
on normal, error, partial-initialization, and panic exits.

## Reference Patterns And Plan

- Codex `codex-rs/tui/src/tui.rs` arms `TerminalInitializationGuard` before
  setup, restores all modes despite individual failures in `restore_common`,
  and restores before chaining the previous panic hook.
- Claude Code `src/utils/gracefulShutdown.ts::cleanupTerminalModes` restores
  terminal modes synchronously before asynchronous shutdown work.
  `src/ink/ink.tsx::detachForShutdown` also restores raw stdin when the normal
  component-unmount path is bypassed.
- Adapt these patterns through a private terminal-mode guard, a separate
  session runner, and isolated PTY subprocess regression checks. Preserve
  runtime error identity and keep mode ownership out of the rendering backend.

## Scope And Key Decisions

- The guard is armed before raw mode, mouse reporting, or paste reporting.
  Every startup and loop error crosses the same restoration boundary.
- The panic hook is installed once and restores synchronously before invoking
  the previous hook only during owner initialization or an owner future poll.
  A thread-local scope is reset on every Pending/yield as well as on unwind:
  a caught worker panic on the same executor thread cannot disable the UI.
  If the owner itself catches a panic, the poll boundary returns an error
  instead of continuing a UI whose modes have already been restored.
  Repeated TUI starts do not accumulate hook wrappers.
- Cleanup attempts mouse, bracketed paste, raw mode, and cursor restoration
  independently and returns the first error. Failed cleanup can be retried by
  Drop; errors are logged and never replace an existing runtime error.
- The renderer no longer owns reporting-mode initialization or teardown.
  Mode restoration happens before memory draining.
- Keyboard enhancement is not enabled by this TUI, so cleanup does not pop an
  unowned keyboard stack. Viewport behavior and suspend/resume remain #925.

## Validation

Focused checks:

```bash
cargo test --locked --lib tui::terminal_modes::tests
cargo test --locked --lib tui::event_loop::tests
cargo fmt --all -- --check
cargo check --locked
cargo clippy --locked --all-targets --no-deps -- -D warnings
git diff --check
```

The cleanup test injects failures at each restoration step and multiple
failures together. Unix PTY subprocesses inspect real kernel termios and
reporting/cursor reset sequences for normal return, loop error, partial setup,
owner panic unwinding, initializer panic, internally caught owner panic, and
repeated acquisition. The previous panic hook observes raw mode disabled for
owner panics. A caught Tokio worker panic on the same current-thread executor
leaves raw mode enabled until the owner exits.

Behavioral RED evidence:

- Moving guard construction after initialization reproduces the original
  startup lifetime gap. The PTY test fails with `partial: kernel terminal
  modes`: raw mode leaves `ECHO` and `ICANON` disabled.
- Returning on the first cleanup error reproduces the original teardown's
  early-return behavior. The failure-injection test observes only `[Mouse]`
  instead of all four restoration actions.
- Both mutations are reverted in the final implementation.
- Review identified that process-global ownership alone also restores modes
  for caught worker panics. The new current-thread PTY worker regression fails
  against that implementation: `previous_hook_raw=false`, followed by an
  assertion failure because the running owner's raw mode was disabled.
  Per-poll ownership and a caught-owner termination boundary fix this case.

Final validation evidence: the focused terminal-mode suite reports three
passing tests and one ignored subprocess fixture; the existing event-loop
plugin-rebuild helper reports one passing test. That helper is not a startup
failure or terminal-restoration integration test. `cargo check`, strict Clippy, formatting,
and diff checks complete without source warnings. The macOS debug test linker
reports its large `__eh_frame` compact-unwind limitation; no linker flags or
repository configuration are changed. Remote CI remains pending.

The PTY checks prove mode restoration; they do not prove shell-prompt placement,
scrollback, terminal resizing, or suspend/resume acceptance.
