# Shell Approval Card Selection

## Summary

- Kept shell approval on the transcript card instead of opening a second picker
  overlay.
- Made the shell approval card directly selectable with keyboard navigation.
- Removed duplicated approval-choice summaries from the live approval card.

## Background

The shell approval flow had drifted into two overlapping surfaces. The active
turn already rendered a full approval card, but pressing `Enter` on an empty
composer opened a separate picker overlay with the same decisions. The card
also repeated the approval-choice summary in addition to the full numbered
options, which made the surface look noisy and inconsistent.

## Scope

- TUI shell approval key handling.
- TUI approval-card rendering for the active turn.
- Shell approval user-facing docs.

## Key Decisions

- Treat the transcript card as the only local shell approval surface.
- Reuse `approval_picker_idx` as the current selection for the card itself.
- Support `Up`/`Down` and `j`/`k` to move the selected approval option when the
  composer is empty.
- Let `Enter` apply the currently selected option directly from the card while
  keeping `1` through `4` as direct shortcuts.
- Reset the selected approval option when a new pending shell approval arrives.

## Validation

- `cargo test tui::tests::empty_submit_keeps_shell_approval_on_card_surface -- --nocapture`
- `cargo test tui::tests::pending_shell_approval_number_shortcuts_work_in_local_and_ssh -- --nocapture`
- `cargo test tui::tests::pending_shell_approval_card_selection_clamps_with_navigation -- --nocapture`
- `cargo test tui::render::cells::cells_tests::active_plan::active_turn_cell_renders_shell_approval_as_interaction_card -- --nocapture`
- `cargo test tui::interaction_text::tests -- --nocapture`
- `cargo check`

## Follow-Ups

- None.
