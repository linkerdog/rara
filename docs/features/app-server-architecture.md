# AppServer Architecture

## Problem

Today `TuiApp` is both the agent's mutation target *and* the renderer's data
source. The agent writes directly into `active_turn.entries`, `committed_turns`,
notices, and overlay state on the same data structure that the TUI renderer
reads, with no structured boundary. Consequences:

- Adding a non-TUI consumer (ACP, Wire, headless benchmarks, print mode) is
  difficult because `TuiApp` is the canonical transcript store.
- Every consumer must understand TUI internals.
- Serialization (JSON-RPC, ACP) is done ad-hoc per consumer instead of at a
  single protocol boundary.

## Goal

Agent output is broadcast as typed Rust objects (not JSON) over a lightweight
event bus. Consumers — TUI, ACP, Wire, print mode — subscribe independently.
JSON serialization happens only at the protocol boundary (ACP server, Wire
server), never internally.

## Architecture

```
agent task (tokio::spawn)
  │  TuiEvent (already exists: mpsc::unbounded_channel)
  │    ├── Transcript { role, message }
  │    ├── Terminal(TerminalEvent)
  │    └── ToolProgress { name, stream, chunk }
  │
  │  typed objects (not JSON): TuringCompleteOutputEvent
  ▼
RuntimeEventBus ──┬── TuiConsumer   (Ratatui, zero-copy)
                  ├── AcpConsumer   (ACP JSON-RPC → IDE)
                  ├── WireServer    (Wire JSON-RPC → external programs)
                  │     └── Print   (rendered as plain text)
                  └── bench / test  (headless assertions)
```

### Key Design Decisions

1. **Internal = objects, external = protocol.** No consumer inside RARA's
   process pays serialization cost. JSON is only emitted by `AcpServer` and
   `WireServer` at the process boundary.

2. **AppServer is just the bus + subscription registry.** It does not know
   about rendering, JSON, or terminals. It accepts subscribers and fans out
   events. That's it — no heavy orchestration.

3. **Consumers are peers.** The TUI is not special. It subscribes the same
   way ACP does. You can run TUI alone, ACP alone, TUI+ACP, or TUI+Wire
   simultaneously.

## `AppServer` (lightweight dispatch layer)

```rust
/// Fan-out bus that accepts typed agent-output events and delivers them
/// to all registered subscribers.
pub struct AppServer {
    /// Each subscriber gets a clone of every event.
    subscribers: Vec<mpsc::UnboundedSender<AgentOutputEvent>>,
}

impl AppServer {
    /// Register a consumer. Returns a receiver the consumer polls from its
    /// own task—no shared state, no locks.
    pub fn subscribe(&mut self) -> mpsc::UnboundedReceiver<AgentOutputEvent> { .. }

    /// Fan out one event to all subscribers. Called by the agent task.
    pub fn publish(&self, event: AgentOutputEvent) { .. }
}
```

`AgentOutputEvent` variants mirror what the agent already emits today:

```rust
pub enum AgentOutputEvent {
    ActiveTurnAppend(TranscriptEntry),
    ActiveTurnReplace(Vec<TranscriptEntry>),
    CommitActiveTurn,
    Notices(Vec<String>),
    Overlay(Overlay),
    OverlayDismiss,
    /// Atomically fan out multiple events (prevents render tearing).
    Batch(Vec<AgentOutputEvent>),
}
```

## Consumers

### TuiConsumer

Replaces the current `TuiApp`-as-global-state pattern. `TuiMaintainer` (added
in PR #276) is the first step — it owns `TuiApp` privately and consumes
`AgentOutputEvent` from its channel. The event loop becomes:

```rust
let rx = app_server.subscribe();
let mut tui = TuiMaintainer::new(TuiApp::new(config)?);

loop {
    tokio::select! {
        event = rx.recv() => {
            tui.apply(event);
            terminal.draw(|f| render(f, tui.state()))?;
        }
        input = terminal_input.next() => {
            tui.handle_input(input).await?;
        }
    }
}
```

### AcpConsumer

ACP mode starts an `AcpServer` that subscribes to the same `AppServer`.
Incoming `PromptRequest` is dispatched to the agent; agent output events
are translated to ACP `SessionNotification` on the wire:

```rust
let rx = app_server.subscribe();
let acp_server = AcpServer::new(rx);

tokio::spawn(async move {
    while let Some(event) = rx.recv().await {
        let notification = acp_server.translate(event);
        acp_server.send_notification(notification).await;
    }
});
acp_server.serve_stdio().await?;
```

### WireServer

Identical pattern to `AcpConsumer` but speaks the Wire JSON-RPC protocol
(Kimi-aligned). A `--wire` mode starts `WireServer::serve_stdio` with the
same `AppServer` subscription. Print mode is a thin `WireConsumer` that
renders `AgentOutputEvent → plain text` instead of JSON-RPC.

### Headless / Bench

Tests subscribe to `AppServer`, run a turn, assert on events — no rendering
cost, no TUI dependency.

## Migration Plan

The migration is incremental. Each step is independently mergeable and green on
tests. Steps 1–2 are done; steps 3–6 are planned.

| # | PR | Change |
|---|----|--------|
| 1 | #276 | Add `TuiMaintainer` struct |
| 2 | #276 | Wire `TuiMaintainer` into `event_loop.rs` via `split_mut()` |
| 3 | follow-up | Publish `TuiEvent` (already the agent output stream) on `RuntimeEventBus` so non-TUI consumers can subscribe. The mpsc channel already exists between agent and TUI; add a `publish` call in `apply_tui_event` / `forward_event_to_bus`. |
| 4 | follow-up | Add `AppServer` with subscribe/publish, move `TuiMaintainer` to consume from it |
| 5 | follow-up | Add `AcpServer` as peer consumer — same subscription, zero TUI code touched |
| 6 | follow-up | Add `WireServer` for external program integration, `--wire` and `--print` CLI flags |
