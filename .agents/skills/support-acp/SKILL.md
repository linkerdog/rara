---
name: support-acp
description: Guide for building external clients (IDE plugins, agents) that integrate with RARA via the Agent Client Protocol (ACP). Use when adding ACP support to an external system, debugging ACP transport issues, or extending RARA's control plane for new protocol clients.
---

# Support ACP — RARA Agent Client Protocol Integration

Use this skill when integrating an external system with RARA over the [Agent Client Protocol
(ACP)](https://github.com/agent-client-protocol/agent-client-protocol) via stdio transport, or
when extending RARA's control-plane surface for new protocol adapters.

## Goals

- Provide a complete, working reference for ACP clients connecting to RARA.
- Document the control-plane protocol that external clients use to manage sessions, submit
  prompts, register sources, and subscribe to events.
- Cover event translation, provenance, and trust boundaries.

## Architecture Overview

RARA implements ACP through these modules:

```
src/acp.rs              — ACP agent impl (stdio transport, session create/prompt)
src/acp_consumer.rs     — subscribes to RuntimeEventBus, translates AgentEvent → SessionNotification
src/control_plane.rs    — routes RuntimeControlEnvelope → domain handlers
src/runtime_control.rs  — full control-plane type system (requests, events, provenance)
src/runtime_event_bus.rs — event bus with raw (AgentEvent) and structured (RuntimeControlEvent) channels
src/protocol_sources.rs — protocol-registered prompt/skill/memory sources
```

### Runtime Architecture

```
External Client (stdio)
     │
     ▼
acp.rs ──────► RuntimeEventBus (send AgentEvent) ──► agent loop
     ▲                          │
     │                          ▼
     │              acp_consumer.rs (subscribe_control)
     │                          │
     │                          ▼
     └──────── SessionNotification (AgentEvent translated)
```

The control-plane channel (`subscribe_control`) carries structured `RuntimeControlEvent` values and
`RuntimeEvent` values for protocol-native events (MCP, hooks, etc.).

---

## 1. Transport

RARA starts ACP in stdio mode:

```
rara acp
```

The protocol uses the `agent-client-protocol` crate (version `0.11`, feature `unstable`) over
stdin/stdout with JSON-RPC framing.

Clients spawn `rara acp` as a child process and communicate over its stdin/stdout.

---

## 2. Session Lifecycle

### 2.1 Create a Session

Send `session/create` with an optional `cwd` and `session_id`:

```json
{
  "method": "session/create",
  "params": {
    "cwd": "/path/to/project",
    "session_id": "optional-session-id"
  }
}
```

RARA responds with a `session/created` notification containing the session context (provider,
model, working directory, bash approval policy).

### 2.2 Resume a Session

```json
{
  "method": "session/resume",
  "params": {
    "session_id": "existing-session-id"
  }
}
```

### 2.3 Cancel / Interrupt

Cancel the current turn or interrupt an in-progress operation:

```
control plane request: SessionControlRequest::CancelCurrentTurn
control plane request: SessionControlRequest::InterruptCurrentTurn
```

---

## 3. Prompt Submission

Send a user prompt via `session/prompt`:

```json
{
  "method": "session/prompt",
  "params": {
    "prompt": "user message text"
  }
}
```

This translates to an `AgentEvent::UserMessage` pushed through the event bus.

---

## 4. Event Translation

The `acp_consumer.rs` subscribes to the structured control bus and translates `AgentEvent`
variants into ACP `SessionNotification` values:

| AgentEvent | SessionNotification | Notes |
|---|---|---|
| `AgentEvent::AssistantMessage` | `assistant/message` | Streaming text is flushed on turn boundary |
| `AgentEvent::ToolCall` | `tool/progress` | Tool name, args, status sent as tool progress |
| `AgentEvent::ThinkingDelta` | `assistant/thinking` | Thinking/reasoning content sent separately |
| `AgentEvent::PlanStep` | `plan/step` | Plan step updates |
| `AgentEvent::PlanApprovalNeeded` | `plan/approval_needed` | Blocking approval request |
| `AgentEvent::ShellApprovalNeeded` | `approval/shell` | Bash command approval |
| `AgentEvent::GoalStatus` | `goal/status` | Goal progress |
| `AgentEvent::Warning` | `warning` | Runtime warnings |
| `AgentEvent::Error` | `error` | Runtime errors |

### Tool Events

Tool lifecycle events are published as structured `ToolEvent` values:

- `ToolEvent::Started` — tool invocation begins
- `ToolEvent::Progress` — incremental output (e.g. streaming shell output)
- `ToolEvent::Completed` — tool finished (contains `ToolResult`)
- `ToolEvent::Errored` — tool invocation failed

Tool output streams are tagged with `ToolStream` variants: `Stdout`, `Stderr`, `System`.

---

## 5. Control Plane Protocol

External clients that need deeper integration can use the control-plane request/event protocol
via the structured bus.

### 5.1 RuntimeControlEnvelope

All control-plane requests are wrapped in a `RuntimeControlEnvelope`:

```rust
struct RuntimeControlEnvelope {
    request_id: String,
    provenance: RuntimeProvenance,
    request: RuntimeControlRequest,
}
```

### 5.2 Provenance and Trust

```rust
struct RuntimeProvenance {
    controller: RuntimeControllerKind,  // Acp, Wire, AppServer, ...
    adapter: Option<String>,
    session_id: Option<String>,
    source_id: Option<String>,
    trust: RuntimeSourceTrust,          // Trusted | Untrusted
    authorship: RuntimeSourceAuthorship, // UserProvided | Generated | Runtime
}
```

ACP adapters connect as `RuntimeControllerKind::Acp` with `RuntimeSourceTrust::Untrusted`.
Trust elevation for specific operations is handled through the approval flow.

### 5.3 Request Categories

```
RuntimeControlRequest
├── Session(SessionControlRequest)       — create, resume, cancel, interrupt, query
├── Input(InputControlRequest)           — submit prompt, answer pending input,
│                                          answer plan approval, answer shell approval
├── Output(OutputSubscriptionRequest)    — subscribe/unsubscribe to output stream
├── PromptSource(PromptSourceControlRequest) — register/unregister prompt sources
├── SkillSource(SkillSourceControlRequest)   — register skill roots/skills
├── Mcp(McpControlRequest)                   — query status, refresh, reconnect
├── Memory(MemoryControlRequest)             — add/update/delete records, list labels
├── Hook(HookControlRequest)                 — hook lifecycle management
└── Approval(ApprovalControlRequest)         — approval policy management
```

### 5.4 Event Categories

```
RuntimeEvent
├── Session(SessionEvent)       — session created, resumed, ended
├── Input(InputEvent)           — prompt received, input answered, follow-up received
├── Assistant(AssistantEvent)   — assistant message, thinking, plan steps
├── Tool(ToolEvent)             — tool started, progress, completed, errored
├── Approval(ApprovalEvent)     — shell approval requested/answered
├── Plan(PlanEvent)             — plan explanation, steps, completed
├── PromptSource(PromptSourceEvent)  — registered, unregistered
├── Skill(SkillEvent)           — skill registered, loaded, disabled
├── Mcp(McpEvent)               — mcp status changed
├── Memory(MemoryEvent)         — memory created, updated, deleted
├── Hook(HookEvent)             — hook lifecycle
├── Context(ContextEvent)       — context observability, budget updates
├── Todo(TodoEvent)             — todo list changes
├── Warning(WarningEvent)       — runtime warnings
└── Error(ErrorEvent)           — runtime errors
```

---

## 6. Protocol-Registered Sources

External clients can register prompt sources, skill sources, and memory records through the
control plane. These sources participate in normal precedence resolution alongside local sources.

### 6.1 Prompt Sources

```rust
PromptSourceControlRequest::Register(PromptSourceRegistration {
    source_id: String,
    scope: SourceScope,         // Home | Repo | Cwd | Session | Protocol
    layer: SourceLayer,         // System | Developer | User | Memory | Skill
    budget_hint_tokens: Option<u32>,
    lifetime: PromptSourceLifetime, // Turns(n) | Session | Persistent
    content: String,
})
```

- Turn-limited sources are automatically expired after the specified number of turns.
- Session sources expire when the session ends.
- Persistent sources need explicit unregistration.

### 6.2 Skill Sources

```rust
SkillSourceControlRequest::RegisterSkill {
    source_id: String,
    name: String,
    content: String,
    precedence_hint: Option<i32>,
}
```

Protocol-registered skills can shadow or extend local skills based on precedence.

### 6.3 Memory Records

```rust
MemoryControlRequest::AddRecord {
    memory_id: String,
    scope: MemoryScope,
    content: String,
    metadata: Value,
}
```

Protocol-registered memory records participate in normal memory retrieval and selection.

---

## 7. Output Subscription

External clients subscribe to the output stream to receive structured events:

```
OutputSubscriptionRequest::Subscribe { subscriber_id }
OutputSubscriptionRequest::Unsubscribe { subscriber_id }
```

Subscribed clients receive all `RuntimeEvent` values published through the control bus.

---

## 8. Implementation Reference: Building an ACP Client

### 8.1 Spawn RARA

```rust
use std::process::{Command, Stdio};

let mut child = Command::new("rara")
    .arg("acp")
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::inherit())
    .spawn()?;
```

### 8.2 Create a Session

Write JSON-RPC to stdin:

```json
{"jsonrpc":"2.0","id":1,"method":"session/create","params":{"cwd":"/path/to/project"}}
```

Read the `session/created` notification from stdout.

### 8.3 Send a Prompt

```json
{"jsonrpc":"2.0","id":2,"method":"session/prompt","params":{"prompt":"Explain this code"}}
```

### 8.4 Handle Events

Read `SessionNotification` values from stdout as JSON-RPC notifications:

```json
{"jsonrpc":"2.0","method":"notifications/send","params":{"sessionNotification":"assistant/message","message":"..."}}
```

### 8.5 Approve a Plan

When the agent enters planning mode:

1. Receive `plan/approval_needed`
2. Send approval or rejection through the input control path:

```json
{"jsonrpc":"2.0","id":3,"method":"input/answer","params":{"type":"plan_approval","approved":true}}
```

### 8.6 Approve a Shell Command

When the agent requests shell approval:

1. Receive `approval/shell` with command details
2. Send decision:

```json
{"jsonrpc":"2.0","id":4,"method":"input/answer","params":{"type":"shell_approval","decision":"once"}}
```

Valid decisions: `once`, `prefix`, `always`, `suggestion`.

---

## 9. Current Limitations

- MCP resource references are not yet wired into the `/context` prompt assembly.
- Tool search for MCP tools is scaffolded but not yet injecting discovered tools into the prompt.
- The `support-acp` integration skill (this file) documents the protocol surface; full end-to-end
  IDE integration demos are not yet available.

## 10. Testing ACP Integration

Run RARA in ACP mode with a test session:

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"session/create","params":{"cwd":"."}}' | rara acp
```

For interactive testing, pipe JSON-RPC commands and observe responses.
