# TUI Paste Input Ordering

## Summary

Issue [#917](https://github.com/linkerdog/rara/issues/917) reports that immediate
submission omits buffered paste and leaves it to appear in the next composer.
The same deferred insertion also changes edit order and routes history keys
against stale empty input. INPUT-02 now names the complete input boundary.

## Reference Patterns And Plan

- Codex `bottom_pane/paste_burst.rs` exposes a pure buffering state machine and
  `flush_before_modified_input`; `chat_composer.rs` applies the buffered text
  through the normal paste path before other input actions.
- Claude Code `hooks/useTextInput.ts::mapKey` inserts a complete normalized
  paste through its cursor abstraction instead of replaying its content as
  shortcut keys. Its paste callbacks deliver the complete string together.
- Codex `chat_composer.rs::set_text_content_with_mention_bindings` clears its
  draft-owned footer flash and pending pastes when replacing composer content.
  Adapt that ownership boundary without clearing unrelated runtime notices.
- Preserve the current burst and large-paste placeholders, flush before key
  routing and direct action dispatch, and clear all pending paste state when
  the user clears input. Verify through the production event/dispatch path.

## Scope And Key Decisions

- Press and Repeat key events flush composer paste before deciding between
  history, cursor movement, pending interactions, and submission. Release
  events remain ignored. Non-composer editors retain their own input.
- Action dispatch also flushes, covering programmatic actions that bypass the
  terminal event source. Forced and timer-driven flushes share normal
  history/palette updates instead of retaining stale history navigation.
- A subsequent small paste first flushes the older burst, preserving event
  order and insertion position.
- Clear removes the input, cursor/scroll state, burst, deadline, and all large
  paste payloads. It also removes the current paste-owned notice, but preserves
  a newer warning/status notice. Track the generated notice instead of matching
  a text prefix that could also describe an unrelated warning. Command-palette
  dismissal uses the same complete clear boundary. Esc keeps its existing
  cancellation/no-op semantics and preserves drafts; it does not introduce a
  new discard gesture.
- `TuiHarness` now uses production terminal-event translation before key
  dispatch, so tests exercise the same pre-routing boundary as the live UI.

## Validation

```bash
cargo test --locked --lib tui::paste_input_tests
cargo test --locked --lib tui::state::composer::tests
cargo test --locked --lib tui::input_ownership_tests
cargo test --locked --lib tui::interaction_tests
cargo test --locked --lib tui::tests::composer_editing
cargo test --locked --lib tui::tests::catalog_and_mouse
cargo check --locked
cargo clippy --locked --all-targets --no-deps -- -D warnings
cargo fmt --all -- --check
git diff --check
```

The six initial behavioral regressions fail against the original implementation: omitted
submission, stale paste after clear, wrong insertion point, empty draft after
Esc, history recall replacing multiline paste, and reversed mixed-size paste
order. The original eight-test paste suite also covers ignored key-release
events and draft-preserving cancellation. The fixture does not advance time
or use wall-clock sleeps.

A production mutation that bypasses the shared update on timed flush also
fails the dedicated history-navigation regression; the mutation is reverted.
The review follow-up reproduces two additional failures on the prior PR head:
the stale paste notice after clear and a pending burst reappearing after palette
dismissal. It broadens clear coverage to small/large and pending/flushed pastes,
adds palette-dismissal and unrelated-warning coverage, and verifies that a
timed flush notice is cleared with its draft. The paste suite now contains ten
tests plus the separate deterministic timer test.

The TUI suite reports 663 passing tests. `cargo check`, strict Clippy,
formatting, and diff checks complete without source warnings. The macOS debug
test linker reports its large `__eh_frame` compact-unwind limitation. Remote
CI for the review follow-up remains pending at this checkpoint. Direct input
replacement in history/palette selection is already preceded by a flush.
Runtime-opened overlays and mouse events may still defer insertion until the
existing timer; their cosmetic timing is unchanged. Unbracketed paste
detection and streaming redraw optimization remain separate work; this fix
does not alter the paste debounce duration.
