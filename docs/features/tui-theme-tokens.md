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
- Adapt the completed frame to the terminal's color and character capabilities.

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

### Terminal Capabilities

The terminal startup boundary detects capabilities once from the environment
and stores the immutable profile on the TUI instance. No terminal queries,
subprocesses, per-frame environment reads, or runtime-global handles are needed.

| Environment | Rendering profile |
| --- | --- |
| Nonempty `NO_COLOR` | Default foreground/background; retain text modifiers |
| `TERM=dumb` | No colors and ASCII display |
| `COLORTERM=truecolor` or `24bit`, or a direct/truecolor TERM | RGB |
| TERM containing `256color` | ANSI 256-color palette |
| TERM containing `16color`, or `linux` | ANSI 16-color palette |
| Other nonempty TERM | ANSI 8-color palette; `vt100`/`vt102` remain monochrome |
| Missing TERM on Windows or with nonempty `WT_SESSION` | RGB |
| Missing TERM without a color or Windows hint | Monochrome |

`NO_COLOR` and `TERM=dumb` take precedence over richer color hints. An empty
`NO_COLOR` does not disable color. Theme overrides describe desired colors;
they do not override the output capability ceiling. The highest-priority
nonempty `LC_ALL`, `LC_CTYPE`, or `LANG` determines encoding. UTF-8/UTF8 enables
Unicode; a non-UTF-8 locale selects ASCII. An absent locale defaults to Unicode
on Windows or with nonempty `WT_SESSION`, and ASCII elsewhere. Explicit TERM
and locale restrictions still apply on Windows. `TERM=dumb` forces ASCII even
with a UTF-8 locale.

The theme owner maps resolved RGB and indexed colors to the nearest supported
nominal palette. ANSI 8/16 output contains only named ANSI colors, and ANSI 256
output contains no RGB colors. If quantization collapses distinct foreground
and background colors, visible text gets a contrasting supported foreground.
No-color output resets colors while preserving bold, dim, reverse, and other
text modifiers.

The terminal adapter encodes named ANSI colors using basic 30-37/40-47 and
90-97/100-107 SGR codes. It does not re-encode ANSI-only output as `38;5;N` or
`48;5;N`, and it does not independently re-read color environment variables.

Capability projection runs on the completed visible frame before terminal
diffing. It covers semantic tokens, legacy colors, syntax highlighting, widget
borders, overlays, and cached transcript rows without changing their sources.
The shared ASCII glyph table maps UI punctuation and symbols to column-sized
alternatives. Other unsupported graphemes use question-mark placeholders of
the same display width. Rendering, cursor columns, and selection coordinates
stay aligned; original input, transcript, and clipboard text remain intact.
This is presentation fallback, not transliteration or a separate line-mode UI;
the interactive TUI still requires cursor-addressing terminal control.

### Theme Configuration

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
- TUI Clippy gates reject raw RGB/indexed constructors and white/black/yellow
  `Stylize` shortcuts outside the theme owner and syntax-color conversion
  boundary. These exceptions preserve configurable themes and highlighted
  source colors; they do not permit raw colors in individual renderers.
- The default theme must preserve the existing Nord-compatible visual baseline.

## Validation Matrix

- Config deserialization accepts `tui.theme.name`, `syntax_theme`, and token
  overrides.
- Theme resolution parses supported color grammars and falls back for invalid
  values.
- Diff, markdown, picker, and overlay renderers compile against semantic token
  lookups rather than direct palette constants.
- Full workspace check and Clippy run without warnings.
- Inject environment maps to verify capability and locale precedence without
  changing process environment during tests.
- Render startup, transcript, highlighted code, diff, sidebar, and overlays at
  each capability level. Assert actual buffer colors/glyphs and text modifiers.
- Exercise production terminal writes for ASCII/color ceilings, wide-cell
  replacement, and cursor alignment. Verify raw input and copied text survive
  display fallback unchanged.

## Open Risks

- The active theme is process-local global state, matching the current TUI
  architecture. A future TUI crate split should pass an explicit theme handle
  into render contexts.
- Syntax highlighting currently selects embedded themes only. Loading external
  `syntect` theme files would need a separate trust and path policy.

## Source Journals

- `docs/journal/2026-07-03-tui-theme-tokens.md`
- [Terminal oracles and lint gates](../journal/2026-10-03-tui-quality-gates.md)
- [Terminal capability fallback](../journal/2026-10-05-terminal-capability-fallback.md)
