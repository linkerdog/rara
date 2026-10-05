# Terminal Feedback

## Summary

Issue #991 adds owned terminal titles and optional unfocused notifications.
The implementation builds on session commands (#1041) and terminal keyboard /
suspend ownership (#1039). Neither dependency PR is merged by this work.

Added `tui.terminal` configuration, stateful window titles and optional BEL/OSC 9
notifications. Live output stays with the event loop; title stack ownership
stays with the terminal guard and its owner-only panic hook. Named thread identity
updates through rename, new thread and prepared restore, with no terminal-time
storage reads. Notification text is fixed and contains no conversation content.

The change preserves runtime/protocol contracts.
Terminal title stacks are the restoration mechanism; title queries would add a
competing input consumer, while clearing a title loses the shell's prior value.

## Reference Adaptation

Local Codex `terminal_title.rs`, `bottom_pane/title_setup.rs`,
`bottom_pane/action_required_title.rs`, `notifications/`, and
`chatwidget/notifications.rs` provide bounded sanitization, typed transports and
attention priority. Local Claude Code `ink/hooks/use-terminal-title.ts` and
`ink/useTerminalNotification.ts` keep writes with the terminal owner and leave
BEL unwrapped under tmux. This implementation preserves those boundaries,
defaults notifications off and restores the previous title instead of clearing
it. The [xterm control-sequence reference](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html)
confirms CSI 22/23 title-stack semantics.

## Key Decisions

- Consume title-stack ownership before attempting restoration, avoiding a second
  pop during Drop after panic-hook cleanup. A completed save arms cleanup before
  flushing; a partial write never claims a parent's stack entry.
- Reuse one multiplexer framing implementation for clipboard and feedback.
  Bell remains unwrapped. Title push/set/pop target the same outer terminal.
- Gate notifications when an event arrives and again before emission. Record
  focused approvals too, so changing focus cannot resurrect them. Approval IDs
  are deduplicated across interleaving within the current query and reset when a
  new query starts. Session IDs fence stale pending output and thread names.
- Query completion waits for both the task result and ordered terminal runtime
  event. Automatic/queued continuations suppress intermediate completion; a
  pending approval wins over completion. Cancel/interrupt and maintenance are
  silent. Restore clears pending feedback and does not signal old decisions.
- Sanitize workspace and thread independently before composing the title, with
  60/120-character bounds. An unterminated escape in one component cannot hide
  the other component. Title updates do not change screen cells or cursor state.

## Validation

- `cargo test --locked --lib tui::`: 1078 passed, four isolated subprocess entry
  points ignored directly but driven by their parent tests. This includes 18
  terminal lifecycle scenarios, title-off/dumb behavior, partial write/flush
  failure, caught owner versus worker panic, two real shell stop/resume cycles,
  ignored suspend, clipboard regressions and rename/new/restore identity.
- `cargo test --locked -p rara-config`: 64 passed, including default omission,
  legacy config and each notification method round trip.
- `cargo test --locked --lib terminal_feedback` and
  `cargo test --locked --lib feedback_tests`: 8 and 2 passed, covering the final interleaved-ID and
  long-path refinements, exact BEL/OSC/title bytes, vt100 cells/cursor stability,
  focus gating, and both completion-barrier orderings.
- `cargo clippy --locked --workspace --all-targets -- -D warnings`,
  `cargo fmt -- --check`, and `git diff --check`: passed.

The first added fixtures exposed invalid setup assumptions: a legacy config
still needs its required provider fields, a scoped approval must follow a turn
start, and a plan completion needs the actual produced-plan state. The fixtures
were corrected to use serialized legacy config and real lifecycle boundaries;
notification assertions were retained. No existing assertions were weakened.

The vt100 model validates invisible control output, not OS delivery or title
stack implementation. Exact save/pop bytes and PTY ordering cover lifecycle;
terminal support and OSC 9 delivery remain emulator/configuration dependent.

## Follow-Ups

No additional scope is planned. Durable decisions are recorded here because the
targeted Nowledge lookup returned `space_client_upgrade_required` (`exact-v1`).

## Dependency Integration

Updated the session-command base while retaining terminal title ownership and
query-failure notifications. The diagnostic guard starts before terminal mode
acquisition; both idle diagnostic/notice tests and feedback tests remain present.
Rename and new-thread completion update typed notices and terminal title state.
The latest main integration also brings the merged keyboard-enhancement and
shared-redaction dependencies into the final validation tree.

Combined TUI validation passed 1,142 tests with seven parent-driven child
fixtures ignored. This includes the real PTY lifecycle, title/keyboard balance,
idle diagnostic delivery, typed notice expiry, and feedback completion barriers.
