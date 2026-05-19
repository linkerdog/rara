# TUI Popup Color-Block Redesign

## Motivation

All overlay popups (help, status, context, command palette, model search,
setup editors, pickers) previously used wireframe `Borders::ALL` /
`Borders::LEFT|RIGHT` blocks on the default terminal background. This created
a dated "DOS dialog" look with visible line-drawing characters around every
pane and input.

[opencode](https://github.com/sst/opencode)'s TUI dialogs use a color-block
approach: a full-screen semi-transparent dimmer behind a solid
`backgroundPanel` color block with no visible borders. This checkpoint mirrors
that design language in RARA.

## What changed

### Theme
- `POPUP_BG` (NORD1 `#3B4252`) — popup content area background
- `POPUP_DIMMER_BG` (NORD0 `#2E3440`) — full-screen dimmer behind centered popups

### Positioning (adaptive sizing)
- `popup_rect(area, max_width, max_height_pct)` replaces `centered_rect`:
  horizontal center, vertical offset at `height / 4` (opencode-style).
  Width/height clamped to `area - 4` so popups never exceed the visible
  terminal.
- `render_dimmer(f, area)` — fills the given area with `POPUP_DIMMER_BG`
  before rendering popup content, creating a visual depth layer.
- `popup_block()` — styled `Block` with `POPUP_BG` background and 1-char
  horizontal padding, no borders.

### Style (border removal)
- All `Block::default().borders(Borders::ALL)` → styled with
  `UI_ELEMENT_BG` / `POPUP_BG` backgrounds + padding
- All `.block(Block::default().borders(Borders::LEFT | Borders::RIGHT))`
  removed from list and paragraph widgets — content flows naturally within
  the panel area
- `command_palette_rect` and `bottom_picker_rect` kept their bottom-anchored
  positioning but got the same color-block treatment

### Files
- `src/tui/theme.rs` — `POPUP_BG`, `POPUP_DIMMER_BG`
- `src/tui/render/overlay.rs` — `popup_rect()`, `render_dimmer()`,
  `popup_block()`; all modal renderers updated
- `src/tui/render/overlay_setup.rs` — all editor/setup modals updated
- `src/tui/list_picker.rs` — picker modals updated

## Trade-offs

- The dimmer overlay is rendered as a full-area color fill, not a true
  transparency. NORD0 is already the main UI background, so the dimmer
  "darkens" the area behind the popup but doesn't show through the prior
  content.
- Snapshot tests were not updated in this change. The visual diff is large
  enough that snapshots should be reviewed and accepted in a follow-up
  after the design direction is confirmed.
- `Borders` type is no longer imported in `overlay`, `overlay_setup`, or
  `list_picker` — any future bordered UI element will need to add the import
  back.

## Remaining

- Update snapshot tests (`cargo insta review`) after design approval
- Consider a thin separator line (NORD3) between sections inside large popups
  if the pure color-block approach loses too much structure
- Command palette and model search still use `command_palette_rect` instead of
  the centered `popup_rect` — evaluate whether these should also become
  centered overlays
