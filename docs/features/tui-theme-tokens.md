# TUI Theme Tokens

## Problem

The TUI had a static Nord palette plus a set of semantic constants, but the
palette was not configurable and several renderers still referenced raw color
constants directly. This made picker visibility, diff colors, markdown colors,
and overlay surfaces hard to tune without changing renderer internals.

## Scope

- Add a structured config surface under `tui.theme`.
- Resolve named semantic theme tokens at render time.
- Keep the existing Nord-compatible palette as the default.
- Route markdown, diff previews, list pickers, command/status/model overlays,
  setup overlays, and popup surfaces through semantic tokens.
- Let the TUI choose the active embedded `syntect` theme through config.

## Non-Goals

- No runtime theme picker or live theme reload command.
- No external theme file format.
- No custom `syntect` theme loading from disk.
- No change to layout, key handling, or picker selection behavior.

## Architecture

`RaraConfig` owns a `tui.theme` object:

```json
{
  "tui": {
    "theme": {
      "name": "nord",
      "syntax_theme": "Nord",
      "tokens": {
        "text.accent": "#88c0d0",
        "picker.highlight.bg": "ansi:12"
      }
    }
  }
}
```

The TUI installs this config when `TuiApp` is constructed. Renderers call
`theme_color(ThemeToken::...)`; unresolved or invalid token overrides fall back
to the default Nord-compatible value and log a warning.

Supported color values:

- `#rrggbb`
- `ansi:N`
- `reset`
- Ratatui color names such as `red`, `dark_gray`, and `light_blue`

`syntax_theme` selects an embedded `two-face`/`syntect` theme by case-insensitive
name. Unknown names fall back to the existing `CatppuccinMocha` default.

## Contracts

- Token keys are stable dotted strings such as `text.accent`,
  `picker.highlight.bg`, `overlay.highlight.bg`, `diff.add.fg`, and
  `markdown.code`.
- A hyphen in config keys is accepted as a dot separator, so
  `picker-highlight-bg` maps to `picker.highlight.bg`.
- Underscores remain significant for token names such as
  `surface.bottom_pane.bg`.
- Invalid token keys and invalid color values are non-fatal and must not break
  TUI startup.
- Renderers should depend on semantic tokens instead of raw palette constants.
- The default theme must preserve the existing Nord-compatible visual baseline.

## Validation Matrix

- Config deserialization accepts `tui.theme.name`, `syntax_theme`, and token
  overrides.
- Theme resolution parses supported color grammars and falls back for invalid
  values.
- Diff, markdown, picker, and overlay renderers compile against semantic token
  lookups rather than direct palette constants.
- Full workspace check and Clippy run without warnings.

## Open Risks

- The active theme is process-local global state, matching the current TUI
  architecture. A future TUI crate split should pass an explicit theme handle
  into render contexts.
- Syntax highlighting currently selects embedded themes only. Loading external
  `syntect` theme files would need a separate trust and path policy.

## Source Journals

- `docs/journal/2026-07-03-tui-theme-tokens.md`
