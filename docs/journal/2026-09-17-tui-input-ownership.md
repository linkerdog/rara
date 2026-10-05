# TUI Input Ownership And Draft Preservation

## Summary

Give model search the same cursor-aware editing boundary as setup fields,
preserve the underlying composer across overlays, and make ordinary j/k input
work at the start of a message. The contract is
[composer and overlays](../interaction/composer-and-overlays.md).

## Reference Review And Plan

- Local Codex source `ea2046f36d5ee12d39c8e168fc3e5129301afa2b`,
  `codex-rs/tui/src/bottom_pane/textarea.rs`: ordinary input and cursor actions
  share one text editor; Vim handling is explicitly enabled. The composer
  keeps draft text/cursor/paste state together.
- Official [Claude Code interactive documentation](https://code.claude.com/docs/en/interactive-mode)
  distinguishes ordinary editing, explicit Vim mode, and draft-preserving
  model selection. The local reconstructed Claude checkout is a third-party
  reference, not authoritative upstream source.
- Reuse existing text/cursor helpers with a model-search target rather than
  adding a separate string editor. Keep field ownership explicit for paste.
- Prove old-code failures through the real key dispatcher and renderer, then
  apply the fix and run focused plus shared TUI checks.

## Scope

No provider protocol, persisted configuration, or terminal key encoding changes.
Existing non-search picker and approval shortcuts remain unchanged. Full Vim
and resume-search cursor editing remain separate work.

The touched 1,571-line state facade and 3,963-line interaction test file need
structural splits to satisfy the repository rules. Move existing bodies without
rewriting their behavior; keep each resulting module below 1,000 lines.

## Validation

- Old-code replay at `4c79bb8f6fea7b71030738587e38b09c4c899cef` with the new
  harness tests compiled and failed all ten cases. Behavioral failures included
  swallowed initial letters, edits applied to the wrong field, discarded drafts,
  paste ownership, and the invisible/scrolled-away query cursor. The CJK cursor
  assertion was subsequently strengthened to inspect physical buffer cells,
  because wide-character continuation cells are not ordinary spaces in a string.
- `cargo test --locked --lib tui::input_ownership_tests`: 10 passed.
- `cargo test --locked --lib tui::`: 623 passed, including the moved tests.
- `cargo check --locked`: passed.
- `cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings`:
  passed.
- The local macOS test linker reports the same pre-existing large `__eh_frame`
  warning as the baseline replay. No new Rust or Clippy diagnostics appeared.
- No snapshots changed. PTY acceptance and the broader terminal matrix remain
  follow-up work; buffer checks do not establish terminal-specific acceptance.
