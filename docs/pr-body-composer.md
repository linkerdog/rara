## Summary

Improves the composer input box for multi-line input and large text pastes, modeled on Codex's `ChatComposer` patterns (live_wrap, paste burst, scroll offset).

## Changes

### 1. Paste Burst Detection

Large (>1000 chars) or multi-line pastes are accumulated in a burst buffer and flushed via a single `push_str` instead of per-char `insert_char`, avoiding O(n²) per-frame redraws from the Ratatui rendering loop. Debounce window: 500ms after last paste event.

| File | Change |
|------|--------|
| `src/tui/state/bottom_pane_model.rs` | `handle_paste_burst_chunk()`, `check_paste_burst_flush()`, `flush_paste_burst()` |
| `src/tui/terminal_ui.rs` | `handle_paste()` routes large pastes to burst |

### 2. Wrapped-Row Scroll

`maintain_composer_scroll` now uses the soft-wrapped row count from the renderer instead of only counting hard newlines. When the input wraps to 10 visual lines but contains only one `\n`, the scroll correctly tracks the visual cursor position.

| File | Change |
|------|--------|
| `src/tui/state/composer.rs` | Signature: `maintain_composer_scroll(width, height, wrapped_cursor_row, wrapped_total_rows)` |
| `src/tui/render/bottom_pane/composer.rs` | `find_cursor_row_in_wrapped()` maps char offset to wrapped row; call site passes wrapped info |

### 3. Wrapping Cache

`wrapped_text_rows()` now has a `thread_local` single-entry cache keyed by `(input, width)`. The same wrapped output is reused across cursor, scroll, and render phases — all three call the function each frame with identical arguments.

| File | Change |
|------|--------|
| `src/tui/render/bottom_pane/composer.rs` | `wrapped_text_rows()` → `wrapped_text_rows_cached()` → `wrapped_text_rows_uncached()` |

### 4. Dynamic Height

`desired_composer_height()` was already implemented; the layout uses `Constraint::Min(3)` to allow the composer to grow beyond 3 lines when the wrapped text exceeds that height.

## Files Changed

| File | +/- |
|------|-----|
| `src/tui/state/bottom_pane_model.rs` | +60 |
| `src/tui/state/composer.rs` | +16 / -10 |
| `src/tui/render/bottom_pane/composer.rs` | +50 / -5 |
| `src/tui/terminal_ui.rs` | +6 / -2 |

## Codex References

- `codex-rs/tui/src/bottom_pane/paste_burst.rs` — paste burst debounce pattern
- `codex-rs/tui/src/live_wrap.rs` — `RowBuilder` incremental wrapping (future improvement)
- `codex-rs/tui/src/bottom_pane/chat_composer.rs` — `desired_height()`, `cursor_pos()`, `scroll_offset`
