# Busy Commands And Permission Controls

## Summary And Plan

Make inspection commands available during work, show disabled reasons for
runtime mutations, and replace opaque permission bookkeeping with effective
policy matching and explicit deferred application. Contracts live in
[commands](../interaction/commands.md) and [permissions](../interaction/permissions.md).

1. Share command availability across discovery and execution, without changing
   the running phase when inspecting state.
2. Centralize preset metadata and policy application. Use the runtime command
   owner for selection; display effective and pending policies separately.
3. Apply pending choices after interpreting the finishing task and before any
   continuation. Preserve approval cards and prove the ordering with regressions.

## Reference Review

- Local Codex `ea2046f36d5ee12d39c8e168fc3e5129301afa2b`,
  `chatwidget/permissions_menu.rs` and `permission_shortcuts.rs`: match effective
  policies, display disabled reasons, and send thread-scoped permission updates
  rather than treating a local label change as application.
- Codex `16f59db96eaa260eb2863974e4e6dea6bcca6bb3`,
  `tui/src/slash_command.rs`: explicitly classifies commands allowed during a
  task, including Permissions and inspection commands.
- Official [Claude Code interaction documentation](https://code.claude.com/docs/en/interactive-mode)
  separates permission modes, dialogs, and in-flight controls. A local third-party
  reconstructed command registry filters availability at use time; it is not
  authoritative Claude source.
- Official [Claude Code CLI reference](https://code.claude.com/docs/en/cli-reference)
  documents `--dangerously-skip-permissions`. Codex's CLI resolves its dangerous
  bypass flag into explicit approval and sandbox overrides. Adapt the explicit
  session opt-in to the existing Full access policy; do not disable OS isolation.
- Codex at the same local revision, `bottom_pane/approval_overlay.rs` and
  `list_selection_view.rs`: measure wrapped content and reserve the action list
  before allowing the header to consume the available height.

## Boundaries And Trade-offs

Keep existing preset capabilities; correct the misleading descriptions. Use
the existing in-process runtime bridge rather than introducing a public ACP or
Wire protocol change. Updates take effect at a task boundary, not halfway through
executing a tool. The second PR depends on the input-ownership module split.

The user also requested an explicit startup bypass. Add the flag to local TUI
and headless session commands, keep `exec --full-access`, and reject unsupported
surfaces. Keep it separate from persisted configuration. TUI startup already
forces network off; retain that default and apply an explicit override only
after restoring the session and before dispatching work.

The reported SSH screenshot also exposed a layout defect: a fixed five-row
approval panel counted logical command lines, then wrapped them during rendering,
which pushed the action row out of view. Reserve actions before preview content,
stack actions at narrow widths, and visibly elide excess preview. Pending
decisions use the full terminal viewport; ordinary history reservation remains
unchanged. Move bottom-pane orchestration out of `mod.rs` into focused layout
and interaction modules while implementing that contract.

## Validation

- Old-code replay: eight initial permission/availability regressions failed
  behaviorally before implementation. The new CLI flag parser regression also
  failed against the old CLI.
- Completion mutation: bypassing pending-policy application made cancellation
  with queued input and automatic-plan continuation fail (two failures, five
  passes). Restore the application boundary before verification.
- Resume replay: restoring a thread did not request reapplication of the active
  Full access policy; the new regression failed before the restore-path fix.
- Screenshot regression: all three approval-layout tests failed against the
  five-row panel. After the fix, every shell choice and the selected marker stay
  visible at 180x28, 80x24, 60x14, and 40x10, using the actual computed viewport.
  Preview elision and full-height pending decisions have separate assertions.
- Full library verification: 1442 passed, one existing ignored test. The first
  sandboxed run could not write the legacy compaction fixtures under `~/.rara`
  or inspect a PTY child. The authorized unrestricted rerun passed all tests.
- CLI tests also run a scripted escalated shell request through a real
  `RuntimeSession`: ordinary startup emits approval without invoking the tool;
  explicit bypass invokes it with network enabled. The fake tool records the
  call without executing a process. Config serialization remains unchanged.
- `cargo check --locked`, strict workspace Clippy, formatting, and whitespace
  checks passed. The macOS linker reports its existing oversized `__eh_frame`
  warning when linking the library test binary; no new Rust/Clippy warnings.
- Two existing snapshots were reviewed for accurate initial Custom policy
  feedback. The final rendering extraction keeps touched source files below
  1000 lines and leaves bottom-pane `mod.rs` as declarations and exports only;
  all 649 TUI tests passed again after the extraction.

These are dispatcher, runtime, and Ratatui-buffer checks. They do not establish
acceptance in the reporter's live SSH terminal. Broader PTY acceptance and the
remaining interaction work stay in [TODO](../todo.md).
