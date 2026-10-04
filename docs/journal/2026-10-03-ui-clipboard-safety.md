# Clipboard Responsiveness And UI Panic Boundaries

## Summary

The #926 checkpoint moves native clipboard helpers off synchronous input
dispatch, bounds terminal clipboard output, and removes the reported panic
sites. The clipboard contract is in the mouse-text-selection specification.

## References And Plan

Inspected Codex `tui/src/clipboard_copy.rs` at
`ea2046f36d5ee12d39c8e168fc3e5129301afa2b` and Claude Code
`src/ink/termio/osc.ts` plus `src/ink/ink.tsx` at
`4b9d30f7953273e567a18eb819f4eddd45fcc877`.

Adapt Codex's pre-encoding 100,000-byte OSC 52 limit, injectable clipboard
boundary, and SSH routing, together with Claude Code's asynchronous helper
execution and two-second deadline. Preserve the existing direct/tmux/screen
framing. Unlike silent best-effort fallbacks, helper failures remain visible
through warning logs and copy notices.

The implementation sequence is clipboard ownership/deadlines first, followed
by the issue's panic-site audit and focused validation. No protocol, persistence,
goal-continuation policy, or general lint-suppression changes are included.

## Implementation

- A session-owned clipboard object starts asynchronous native work and retains
  at most one active request and the latest pending request. The existing
  maintenance tick consumes only completed tasks. Superseded results do not
  replace the latest notice, and helper ordering prevents an older native
  copy from completing after a newer native copy.
- One deadline covers native input writes, process waits, and fallback attempts.
  Dropping the timed-out future or the session explicitly terminates the owned
  helper and schedules an asynchronous waiter to reap it. Tokio's
  `kill_on_drop` remains a shutdown fallback. Nonzero exit codes are failures.
- OSC 52 validates UTF-8 bytes before encoding and writes no sequence on overflow.
  Local native helpers still receive the complete text. SSH sends only to the
  terminal clipboard; its notice describes a request rather than confirmed
  terminal acceptance. Native helper success has separate feedback.
- The known global scroll mutexes are replaced with numeric session state,
  preserving the acceleration thresholds while removing poisoning and
  cross-session coupling.

## Panic Audit

| Reported site | Result |
| --- | --- |
| Scroll-state mutex `unwrap` | Removed with session-local acceleration |
| Goal follow-up goal/agent `expect` and inactive-state `unreachable` | Return errors; the existing goal-command boundary logs a warning and displays failure without exiting the UI |
| Palette registration `expect` | Log a warning and render an unavailable entry if registration and parsing disagree |
| Composer pending-interaction `unwrap` | Keep the original filtered optional value instead of checking and fetching it twice |
| Startup runtime-agent `expect` | Propagate a startup error through the existing terminal-mode guard |
| Adjacent MCP cache refresh mutex `unwrap` | The spawned refresh task logs a poisoned-cache warning and stops without panicking |

Workspace lint settings deny `unwrap_used` and `expect_used`, but `src/lib.rs`
has grandfathered allows inherited by the TUI. This explains why the old sites
compiled; it does not establish that every panic is safe. TUI lint enforcement
remains the separate #927 work. Existing terminal-owner cleanup tests continue
to cover error and panic restoration.

## Validation

Before the fix, focused regressions proved that a helper exiting with code 17
was reported as success and a second session inherited wheel acceleration from
the first. New tests use an injected native backend and terminal writer, with
isolated subprocess helpers for stalled input and process-exit cleanup. They
never write to the real native clipboard.

- `cargo test --locked --lib tui:: -- --nocapture`: 918 passed, three ignored
  subprocess fixtures exercised by their parent tests.
- `bazel test //:rara_unit_tests --test_arg=tui::`: passed with the default
  configuration and the same TUI test coverage. A missing external rules cache
  was restored with a scoped `bazel fetch --force //:rara_unit_tests`; no Bazel
  configuration or BUILD files changed.
- `cargo clippy --locked --all-targets --no-deps -- -D warnings`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.

## Reaping Follow-Up

Subsequent full-suite and isolated runs reproduced a timeout-cleanup failure:
the child was still observable after the existing two-second reaping bound.
Tokio's drop path places unfinished children into a best-effort orphan queue;
that is insufficient as the only cleanup owner here. The helper now sends the
kill request synchronously on cancellation and transfers the child to an
explicit asynchronous waiter. Normal completed waits disarm that cleanup.
The existing reaping assertion remains unchanged, and the process fixture also
covers session drop while the helper stalls on stdin or exit. The focused
replay passed, followed by five repetitions covering 20 helper lifecycles.
Cargo and default Bazel TUI suites again passed all 918 tests, with three
ignored subprocess fixtures exercised by parents. Strict all-target Clippy
also passed on the follow-up.

## Remaining Limits

Terminal output remains owned by the UI thread and bounded by the OSC 52 cap;
this change does not make all terminal writes nonblocking. Terminal policy and
native clipboard availability still require real environment acceptance.
Native helper subprocesses may intentionally daemonize to serve clipboard
contents after successful exit; the deadline covers the helper we spawned.
