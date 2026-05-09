# Mouse Text Selection

## Problem

When `EnableMouseCapture` is active (added in PR #317 for scroll acceleration),
the terminal sends all mouse events to the application. Native text selection
(drag-to-select, double-click to select word) is blocked at the terminal layer.
Users cannot select text from the transcript or input area with the mouse.

This is inherent in all terminal applications that enable mouse reporting; it
is not a Ratatui limitation. The trade-off is:

- Mouse reporting ON  → scroll acceleration, click-to-interact, but no text selection
- Mouse reporting OFF → native text selection, but no mouse-driven scroll/click

## Solution: Shift-Toggle Mouse Passthrough

Hold `Shift` to temporarily disable mouse capture while the key is held,
allowing native text selection. When `Shift` is released, mouse capture resumes.

### Implementation

1. Detect `Shift` key press/release in the key event handler
2. On `Shift` press: `DisableMouseCapture`
3. On `Shift` release: `EnableMouseCapture`

```rust
// event_dispatch.rs
AppEvent::Key(KeyEvent { code: KeyCode::Key(..), modifiers, kind }) => {
    if kind == KeyEventKind::Press && modifiers.intersects(KeyModifiers::SHIFT) {
        execute!(backend, DisableMouseCapture)?;
    } else if kind == KeyEventKind::Release && !modifiers.intersects(KeyModifiers::SHIFT) {
        execute!(backend, EnableMouseCapture)?;
    }
}
```

### Detection

`crossterm` exposes `KeyModifiers::SHIFT` for regular keys. For standalone
Shift press, terminals may send different key events. The reliable approach is
to track `SHIFT` modifier on keyboard events rather than detecting a standalone
Shift press. When ANY key is pressed with Shift held, enter selection mode.

Simultaneously, scan for `MouseEventKind::Down` events and if they occur without
a registered TUI interaction target, treat them as selection start.

### Behavior Matrix

| State | Mouse scroll | Text selection | Mouse click |
|-------|-------------|----------------|-------------|
| Normal (mouse capture ON) | ✅ scroll acceleration | ❌ blocked | ✅ interact with TUI |
| Shift held (mouse capture OFF) | ❌ terminal handles | ✅ native selection | ❌ ignored |

### Constraints

- The backend reference must be accessible from `event_dispatch`. Currently
  `event_dispatch` receives `&mut TuiApp` and `agent_slot`. We need to add
  `&mut Terminal<CrosstermBackend<Stdout>>` or a channel to send capture
  toggle commands.
- `DisableMouseCapture`/`EnableMouseCapture` must be called on the same
  backend that owns the terminal output handle.

## Non-Goals

- Per-pixel mouse selection precision (inherently limited by terminal cell grid).
- Copy-to-clipboard integration (use terminal's native copy: Cmd+C / Ctrl+Shift+C).

## Verification

- Manual: open RARA, hold Shift, drag mouse over transcript text → text should
  be highlighted by the terminal.
- Release Shift, scroll with mouse wheel → scroll acceleration should resume.
