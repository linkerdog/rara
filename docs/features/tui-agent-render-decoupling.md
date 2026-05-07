# TUI Agent/Render Decoupling

## Problem

Today `TuiApp` is both the agent's mutation target *and* the renderer's data
source. The agent writes directly into `active_turn.entries`, `committed_turns`,
notices, and overlay state on the same `Arc<TuiApp>` that the renderer reads,
with no explicit ownership boundary. Consequences:

- Adding a non-TUI transcript consumer (ACP, Wire, headless benchmarks) is
  difficult because TUI state is the canonical transcript store.
- The renderer must defensively skip draw cycles when state hasn't changed
  (the `dirty` flag), but the flag is a coarse heuristic because there's
  no structured "what changed" signal.
- Post-exit resume must reconstruct `TuiApp` from raw transcript files and
  re-derive committed/render-cache state.

The goal of this spec is to separate *agent output* from *render state* so that:

1. The agent writes structured events into a TUI-agnostic event queue.
2. The TUI maintainer consumes those events into a private render model.
3. Future non-TUI consumers (ACP, Wire) can reuse the same event stream.

## Non-Goals (this PR)

- Replacing `crossterm` with a different backend.
- Moving transcript persistence out of `SessionManager` / `ThreadStore`.
- Changing the Ratatui widget tree layout.

## Design

### Agent → Event Bus

After step 1 (PR #272) all rendering goes through `terminal.draw()`. The agent
currently mutates `TuiApp` through `finish_running_task_if_ready()` and
`dispatch_event()`. We convert those mutations into `AgentOutputEvent` enums
sent over the existing `RuntimeEventBus` (or a new TUI-local channel).

```
agent (tokio::spawn)
  │
  ├─── mpsc::Sender<AgentOutputEvent> ──► TuiMaintainer
  │
  ▼
RuntimeEventBus::publish_control(RuntimeEvent::AgentOutput(...))
```

`AgentOutputEvent` variants mirror the current mutation surface:

```rust
pub enum AgentOutputEvent {
    /// Append a content block (text delta, tool call, tool result) to the
    /// active turn.
    ActiveTurnAppend(TranscriptEntry),
    /// Replace the entire active turn (e.g. on new prompt).
    ActiveTurnReplace(Vec<TranscriptEntry>),
    /// Finalize the active turn: move it to committed_turns, clear active.
    CommitActiveTurn,
    /// Push a user-facing notice (errors, status messages).
    Notice(String),
    /// Request an approval/picker overlay.
    Overlay(Overlay),
    /// Dismiss the current overlay.
    OverlayDismiss,
    /// Batch of events emitted atomically (prevents render tearing).
    Batch(Vec<AgentOutputEvent>),
}
```

### TuiMaintainer

A new `TuiMaintainer` owns all mutable TUI state (the current `TuiApp` fields
that were previously shared). It runs in the TUI task, has no `Arc` wrapper,
and is the *only* writer to the render model.

```rust
pub struct TuiMaintainer {
    state: TuiApp,                    // now private, not Arc-wrapped
    agent_events: mpsc::Receiver<AgentOutputEvent>,
    input_tx: mpsc::Sender<InputAction>,
}

impl TuiMaintainer {
    /// Returns the next frame delta (Some) or None if no redraw needed.
    async fn next_delta(&mut self) -> FrameDelta {
        loop {
            tokio::select! {
                event = self.agent_events.recv() => {
                    self.apply_event(event);
                    return FrameDelta::Redraw;
                }
                _ = self.tick.tick() => {
                    // Idle tick — no agent events, no input.
                    // Return NoOp to skip the draw.
                    return FrameDelta::NoOp;
                }
            }
        }
    }
}
```

The event loop simplifies to:

```rust
loop {
    match maintainer.next_delta().await {
        FrameDelta::NoOp => continue,
        FrameDelta::Redraw => {
            terminal.draw(|f| render(f, maintainer.state()))?;
        }
    }
    // Handle input events here via select! with input channel
}
```

### Separation of concerns

| Concern | Before | After |
|---------|--------|-------|
| Agent writes | Directly into `TuiApp` | Pushes `AgentOutputEvent` |
| TUI state | `Arc<TuiApp>` shared | `TuiMaintainer` owns it |
| Render reads | `&TuiApp` direct | `maintainer.state()` snapshot |
| Dirty detection | Global `bool` flag | Per-event enum variant |
| Future ACP consumer | Must reconstruct `TuiApp` | Reads same `AgentOutputEvent` stream |

## Implementation Plan

1. **Define `AgentOutputEvent`** in `src/tui/runtime/events.rs`.
2. **Create `TuiMaintainer`** in `src/tui/maintainer.rs` — owns `TuiApp`, consumes `AgentOutputEvent`.
3. **Refactor `finish_running_task_if_ready()`** to emit `AgentOutputEvent` instead of mutating `TuiApp`.
4. **Refactor `dispatch_event()`** to emit input events through a channel to `TuiMaintainer`.
5. **Simplify `event_loop.rs`** — poll agent events + input channel, call `maintainer.next_delta()`.
6. **Remove `Arc<TuiApp>`** — `TuiMaintainer` owns it exclusively.

## Migration Safety

- This is an internal refactor; no user-visible behavior change.
- All 939 existing tests must pass at each commit.
- The PR is structured as 3–5 incremental commits, each independently green.
- Commit 1 adds types, Commit 2 adds TuiMaintainer, Commit 3 wires event loop,
  Commit 4 removes Arc, Commit 5 cleans up dead code.
