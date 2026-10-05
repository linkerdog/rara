# External Composer Editor

## Summary

Issue #993 adds Ctrl+G to edit the expanded composer in VISUAL/EDITOR. Successful
edits return through paste sanitization without submitting. Failures and unchanged
content preserve the original cursor, file references, and collapsed paste ownership.

The branch integrates #1043's atomic draft/history model with #1042's terminal
ownership and feedback. Local merge `9873722b` preserves both dependencies; no
GitHub PR is merged. Its integration baseline has 1,114 passing TUI tests and
four parent-driven child entry points.

## Reference Adaptation

Codex `external_editor.rs` resolves VISUAL/EDITOR, parses argv, passes the file
as a separate argument, and checks exit status before readback. Claude Code
`utils/promptEditor.ts` expands paste ownership before editing and suspends
stdin/rendering around terminal ownership, restoring it in a finally boundary.
Both patterns fit the inline renderer and asynchronous runtime.

## Key Decisions

- Parse POSIX command quoting with the already locked shlex package, now a
  direct dependency. Do not interpolate a shell command; GUI editors supply
  their own wait option. A private directory owns the Markdown file and editor
  backups. No Bazel configuration changes are needed.
- Keep the original draft until successful readback. Fence the result by session,
  workspace, and draft; a stale successful edit receives a private recovery file
  with a notice instead of overwriting the current draft.
- Release the real event reader and mode guard before process startup. Pump
  runtime events while pausing all presentation, input, and mode maintenance.
  Reacquire modes/input and repaint at the current size on return.
- On Unix, restore captured cooked termios even when an editor leaves raw mode.
  Scoped parent SIGINT/SIGQUIT dispositions leave Ctrl+C to the editor, which
  resets those dispositions before exec. The title stack protects the shell
  title from editor changes. Cancellation drops the child and restores cooked
  modes, the primary screen, and title through the same ownership guard.

## Validation

- `cargo test --locked --lib tui::external_editor::`: 10 passed, two parent-driven
  fixture entry points. The PTY parent covers success, missing configuration,
  cancellation, nonzero exit, spawn failure, readback failure, leaked raw mode,
  and Ctrl+C. It checks input ownership, resize, balanced keyboard/title stacks,
  kernel termios, and temporary-directory cleanup.
- Removing the successful-exit gate makes the nonzero-exit regression fail;
  restoring the gate passes the same test.
- `cargo test --locked --lib tui::`: 1,125 passed, six parent-driven fixture entry
  points. This includes the production event loop with a gated editor: runtime
  commands drain while output and mode maintenance remain paused.
- `cargo clippy --locked --workspace --all-targets -- -D warnings`,
  `cargo fmt --check`, and `git diff --check` pass. Touched Rust source files
  remain below 1,000 lines.

The cancellation fixture uses a local Unix socket to release the pinned editor
future after the child has changed terminal modes. Local sandbox policy rejects
socket creation, so the PTY/full TUI runs use the authorized test environment.
Only fixture executables are launched; the user's configured editor is not used.
Native Windows terminal handoff is outside the Unix PTY evidence.

The targeted Nowledge lookup returned `space_client_upgrade_required`
(`exact-v1`); durable decisions remain in this journal and the feature spec.

## Follow-Ups

No implementation follow-up remains for #993. Remote CI is the acceptance gate
for the published branch, including the default Bazel build and tests.
