# 2026-05-11-thinking-display-enhancement

Replaced flat `# Thinking` sections with collapsible thinking blocks,
blending Claude Code and OpenCode patterns.

## What changed

- **thinking_cells.rs**: `ThinkingBlockCell` merges old `ThinkingTextCell` +
  `ThinkingGroupCell`. Streaming mode shows expanded content with `┊` accent
  bars. Committed mode shows collapsed summary with token estimate.
- **progress.rs**: all live/committed thinking paths use `ThinkingBlockCell`.
- **mod.rs**: exports updated.

## Visual design

| Element | Color | Source |
|---------|-------|--------|
| Header `▸/▾ Thinking · N lines` | TEXT_SECONDARY | Claude Code |
| Accent bar `┊` | TEXT_MUTED | OpenCode |
| Body content | TEXT_MUTED | OpenCode (mimics 0.6 opacity) |
| Collapsed summary | TEXT_SECONDARY | Claude Code |

## Behavior

- Streaming: always expanded (▾), shows content with ┊ accent bars
- Committed: collapsed (▸), one-line summary with line count + token estimate
- Content color uses `span.style.patch()` to preserve markdown formatting
  (bold, italic, code) while shifting to dimmer foreground

## Follow-up

- Expand/collapse toggle on Enter/Click (current: committed always collapsed,
  streaming always expanded)
- Runtime elapsed time in summary line
