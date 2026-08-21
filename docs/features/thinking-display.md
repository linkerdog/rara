# Thinking Display Enhancement

## Motivation

RARA previously rendered thinking as `# Thinking` section header with full inline text — no collapse toggle, no summary, no visual distinction from the rest of the agent response. Both Claude Code and OpenCode provide richer thinking displays with collapsible blocks, dimmed styling, and summary lines.

## Design (learning from Claude Code + OpenCode)

### Claude Code patterns
- `⟐ Thinking (1.2s, 340 tokens)  [Ctrl-O to expand]` — collapsed summary
- Collapsed by default, Ctrl-O toggles expand
- Expanded: dimmed/distinct color for full thinking block
- Configurable via `showThinking`: `"collapsed"` | `"expanded"` | `"hidden"`

### OpenCode patterns
- `▸` / `▾` toggle prefix for collapsible reasoning blocks
- Border-left accent (`┊ `) for thinking content
- Dimmed/italic text for reasoning content
- Thinking block sits between user prompt and agent response sections

### RARA design (blending both)

1. **Collapsible block** — `▾ Thinking · 12 lines` (streaming) / `▸ Thinking · 12 lines (≈45 tokens)` (committed)
2. **Dimmed border-left accent** (`┊ `) for expanded thinking lines
3. **Auto-expand** during live streaming; always collapsed for committed
4. **Unify** `ThinkingTextCell` + `ThinkingGroupCell` → single `ThinkingBlockCell`
5. **Chronological closure** — only the currently open thinking stream is kept
   outside the transcript; closing it inserts a `Thinking` entry at that exact
   event boundary before assistant text, tools, or later progress

## Display state

```
Streaming (expanded):
  ▾ Thinking · 12 lines
  ┊ First line of thinking content
  ┊ Second line of thinking
  ┊ ...

Committed (collapsed):
  ▸ Thinking · 12 lines (≈45 tokens)
```

## Color scheme

| Element | Color |
|---------|-------|
| `▾` / `▸` + `Thinking · N lines` header | TEXT_SECONDARY |
| `┊` accent bars | TEXT_MUTED |
| Thinking body content | TEXT_MUTED |

## Files

- `src/tui/render/cells/thinking_cells.rs` — `ThinkingBlockCell` rewrite
- `src/tui/render/cells/progress.rs` — callers updated
- `src/tui/render/cells/mod.rs` — export updated
- `src/tui/theme.rs` — no new colors needed (reuses TEXT_SECONDARY, TEXT_MUTED)

## Verification

- `cargo fmt` / `cargo clippy` — zero new errors
- state tests cover thinking-before-answer and interleaved assistant/progress
  segments
- active and committed render tests verify that closed and streaming thinking
  remain at their recorded positions
- controller tests cover both task-first and terminal-event-first completion

## Follow-up (planned, not implemented)

- **Enter-key toggle**: committed thinking blocks currently always collapsed. Adding an
  `expanded` state independent of `is_streaming` would enable Enter to toggle.
  Needs plumbing through `input_control.rs` and `terminal_ui.rs`.
- **Runtime elapsed time**: display elapsed duration in the collapsed summary line.
