# AppServer Architecture

## Problem Revisited

The agent already broadcasts `AgentEvent` through `RuntimeEventBus` via
`forward_event_to_bus`. `AgentEvent` contains all the structured output the
agent loop produces: `AssistantText`, `AssistantDelta`, `ToolUse`,
`ToolResult`, `ToolProgress`, `TodoUpdated`, `Status`, etc.

`TuiMaintainer` already owns `TuiApp` privately (PR #276). The TUI is one
consumer among many; the event bus is the canonical fan-out mechanism.

**What's missing**: non-TUI consumers. ACP, Wire, and print-mode stubs exist
but don't subscribe to `RuntimeEventBus`.

## Architecture (current state + planned)

```
agent (tokio::spawn)
  │
  │  AgentEvent (typed, no JSON)
  ▼
RuntimeEventBus ──┬── TuiMaintainer   (Ratatui, via mpsc + event_bus)
                  ├── [planned] Acp   (ACP JSON-RPC → IDE)
                  ├── [planned] Wire  (Wire JSON-RPC → external programs)
                  └── [planned] Print (agent events → plain text)
```

### What Already Works

1. `forward_event_to_bus` publishes every `AgentEvent` to the bus.
   Callers: `start_query_task`, `start_compact_task`, `start_review_task`.
   See `src/tui/runtime/tasks.rs:36`.

2. `RuntimeEventBus::subscribe_control()` returns an `mpsc::UnboundedReceiver`
   for any consumer. No shared state, no locks.

3. The TUI consumes both the mpsc channel (`TuiEvent`) AND the event bus
   (`AgentEvent` on `subscribe_control`) — it's already a dual consumer.

### Adding a Non-TUI Consumer

Any consumer subscribes the same way:

```rust
let rx = event_bus.subscribe_control();
while let Some(envelope) = rx.recv().await {
    match &envelope.event {
        RuntimeEvent::Agent(event) => {
            // event is AgentEvent::AssistantText(...)
            translate_and_send_to_acp(event);
        }
        _ => {}
    }
}
```

No JSON serialization inside the process. Serialization happens only at
the consumer boundary (ACP/Wire).

## Concrete Plan

| # | PR | Change |
|---|----|--------|
| done | #276 | `TuiMaintainer` owns `TuiApp`, event loop uses `split_mut()` |
| done | #280, #281 | ACP publishes `AgentEvent` to `RuntimeEventBus`; injection via `Arc<Self>` |
| done | #286 | `PrintConsumer` — plain text, `--print` CLI |
| superseded | #287 | Removed the unused `AcpConsumer`; ACP now maps runtime-control events directly in the session adapter |
| done | #289 | `WireConsumer` — Wire JSON lines, `--wire` CLI |
