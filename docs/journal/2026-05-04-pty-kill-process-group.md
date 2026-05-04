# 2026-05-04 · PTY Kill Process Groups

## Summary

RARA now treats PTY termination as an asynchronous stop request instead of an
immediate final state.

## What Changed

- Added a `killing` PTY session state. A stop request moves a running session to
  `killing`; the reader thread moves it to `killed` only after PTY EOF.
- On Unix, PTY stop now resolves the child process group from the PTY child pid,
  then sends `SIGKILL` to that process group before falling back to the direct
  child handle. This covers shell-spawned background children that would
  otherwise keep running after the foreground shell exits.
- Completed sessions remain `completed` if a later stop request is submitted,
  avoiding stale pid/process-group kill attempts.
- Failed stop requests restore the session to `running` when the reader has not
  already observed EOF, so callers can retry or inspect the session reliably.
- Signal handling uses `nix` wrappers instead of direct unsafe `libc` calls.

## Validation

- `cargo test pty_kill -- --nocapture`
- `cargo test pty_sessions_can_be_listed_statused_and_stopped -- --nocapture`
- `cargo check`
