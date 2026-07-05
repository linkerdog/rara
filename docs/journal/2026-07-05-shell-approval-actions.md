# Shell Approval Action Visibility

## Summary

The shell approval dock now keeps its action row visible when the approval
detail includes command and working-directory lines.

## Background

The bottom-pane interaction panel has a fixed five-row height. Shell approval
details could fill those rows before the action buttons were rendered, so the
user could see the pending command but not the selectable approval items.

## Scope

- Keep detail text compact inside the dock panel.
- Render approval actions as a fixed final row when detail text is present.
- Add a render regression test for a standard-height terminal with a pending
  shell approval.

## Validation

```bash
cargo test --locked tui::render::tests::shell_approval_panel_keeps_actions_visible
cargo test --locked tui::render::tests
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets -- -D warnings
```
