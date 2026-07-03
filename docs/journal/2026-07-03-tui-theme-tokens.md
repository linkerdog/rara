# 2026-07-03 TUI Theme Tokens

## What Changed

- Added `tui.theme` config for the TUI theme name, semantic token overrides,
  and the active embedded syntax highlighting theme.
- Introduced a semantic `ThemeToken` resolver with Nord-compatible defaults and
  non-fatal fallback behavior for unknown tokens or invalid colors.
- Routed markdown, diff preview, list picker, command/status/model overlays,
  setup overlays, popup panels, and dimmer surfaces through semantic tokens.
- Made syntax highlighting select an embedded `syntect` theme from
  `tui.theme.syntax_theme`.

## Why

The previous palette had static constants that could not be configured, while
some UI paths still used direct colors. That made the TUI hard to tune and left
theme-related TODOs partially implemented. The new token layer gives renderers a
stable semantic contract while preserving the current Nord baseline.

## Trade-Offs

- The theme is still installed into process-local global state because current
  TUI render functions do not carry an explicit render context.
- Unknown config is accepted with warnings rather than failing startup, because
  visual customization should not make the terminal unusable.
- Syntax themes are limited to embedded `two-face` themes for now.

## Verification

- `cargo test -p rara-config loads_tui_theme_config`
- `cargo test --bin rara tui::theme::tests`
- `cargo check --locked --workspace --all-targets`
- `cargo clippy --locked --workspace --all-targets -- -D warnings`

## Remaining Work

- Add a runtime command or settings UI if users need live theme switching.
- Revisit explicit theme handles during the future TUI crate split.
