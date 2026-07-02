# List Picker Visibility

## Summary

RARA list pickers now render with a stateful Ratatui list so selected items stay
visible when the selection moves beyond the first viewport.

## Background

The generic setup/model picker rendered a plain `List` with per-item styles but
without `ListState`. When the selected row was outside the visible area, Ratatui
kept the list offset at zero, which made the active item appear missing.

## Scope

- Added semantic picker foreground and highlight theme tokens.
- Switched generic list picker rendering to `render_stateful_widget`.
- Reused the same highlight style for resume and generic pickers.
- Centralized the resume picker list view so rendering, header counts, and
  resume selection all use the same filtered thread set.
- Added focused coverage for list offset behavior.

## Validation

```bash
cargo test list_picker_state_scrolls_to_selected_item -- --nocapture
cargo test tui::list_picker::tests -- --nocapture
INSTA_UPDATE=always cargo test provider_picker_renders_as_full_overlay_on_standard_terminal -- --nocapture
INSTA_UPDATE=always cargo test unified_model_picker_snapshot -- --nocapture
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings
```

## Follow-Ups

- The broader configurable theme token schema remains open.
- The setup flow can still be consolidated with the model picker in a later
  TUI/UX slice.
