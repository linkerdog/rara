# Runtime Control Plane

## Problem

RARA is growing several runtime capabilities that need to be controlled by both
local surfaces and external applications:

- skills;
- memory;
- prompt sources;
- hooks;
- planning and approvals;
- input submission and cancellation;
- output and transcript streaming.

If each feature is implemented only for the TUI or only for a local command,
later ACP, Wire, AgentHub, or appserver integration will need separate ad hoc
paths for the same runtime state. That would make prompt assembly unstable,
permission decisions harder to audit, and `/context` inconsistent with external
protocol views.

RARA therefore needs a protocol-neutral control plane before these features
become deeply coupled to one UI.

## Scope

This spec defines the architecture boundary for a runtime control plane that can
be driven by:

- the local TUI;
- CLI commands;
- ACP;
- Wire protocol adapters;
- future HTTP, MCP, or appserver integrations.

The control plane owns structured requests and events for:

- session lifecycle;
- user input and pending-interaction answers;
- output subscriptions;
- prompt-source registration;
- skill-source registration and invocation;
- memory mutations, queries, and selection inspection;
- hook declaration and lifecycle dispatch;
- approval and permission decisions.

## Non-Goals

- Implementing full ACP or Wire protocol support in the first slice.
- Letting external applications bypass RARA sandbox, approval, or tool policy.
- Letting third-party protocol adapters concatenate raw text directly into the
  final system prompt.
- Executing arbitrary hook files as soon as they are discovered.
- Replacing the TUI runtime with a protocol server.

## Architecture

### Layering

The target shape is:

1. protocol adapters;
2. runtime control plane;
3. runtime domains;
4. execution and display surfaces.

Protocol adapters include ACP, Wire, local TUI commands, and future appserver
entrypoints. They translate external protocol messages into RARA-owned control
requests and translate RARA events back into protocol-specific responses.

The runtime control plane is protocol-neutral. It validates requests, attaches
provenance, routes to the right domain, records lifecycle events, and emits
structured output events.

Runtime domains remain the source of truth:

- `PromptRuntime`;
- `SkillRegistry`;
- `MemoryStore` and `MemorySelection`;
- `HookRuntime`;
- `ThreadStore` and `ThreadRecorder`;
- approval and sandbox policy;
- transcript and output event streams.

Execution surfaces such as the TUI, ACP worker mode, AgentHub team mode, and
headless evaluation read the same domain state and event stream.

### Control-Plane-Ready Rule

New features that touch skills, memory, prompt sources, hooks, planning,
approval, tool output, `/context`, or `/status` must be implemented so they can
later be controlled through ACP or Wire without duplicating local-only code.

That means:

- core logic lives in runtime or domain modules, not directly in TUI widgets;
- inputs and outputs use structured request and event objects;
- every external or local source carries provenance;
- prompt-affecting data enters through source objects, not string append points;
- visible runtime state can be consumed by `/context`, `/status`, and protocol
  subscribers;
- sandbox, approval, and permission decisions remain centralized in RARA;
- new state is either persisted or explicitly marked transient.

### Provenance

Every control-plane source must carry provenance:

- `local_tui`;
- `local_cli`;
- `home`;
- `repo`;
- `protocol`;
- `runtime`;
- later: `plugin`.

Protocol provenance should also include:

- adapter name, such as `acp` or `wire`;
- session id when available;
- source id or connection id when available;
- whether the source is trusted, user-provided, or generated.

Provenance is required for explainability and safety. `/context`, `/status`,
and protocol output must be able to show where a prompt source, skill, memory
record, or hook came from.

## Control Requests

The first typed request set should include these families.

### Session Control

- create session;
- resume session;
- cancel current turn;
- interrupt current turn;
- query current runtime state.

### Input Control

- submit user prompt;
- answer pending request input;
- answer plan approval;
- answer shell approval;
- submit follow-up while a turn is running.

Input control must use the same pending-interaction state as the TUI. Protocol
adapters must not invent parallel approval or question state.

Submitting a follow-up while a turn is running is a queued input operation by
default. It does not imply cancellation or interruption. Runtime adapters should
preserve submission order and deliver queued follow-ups after the active turn
reaches a safe continuation boundary or completes. If queue policy or capacity
prevents accepting the follow-up, the runtime should return a busy or rejected
status with a structured reason. Adapters that need preemption must use the
explicit cancel or interrupt request family instead of overloading follow-up
submission.

### Output Subscription

External applications should subscribe to structured runtime events:

- assistant text delta;
- assistant thinking delta;
- tool use;
- tool progress;
- tool result;
- approval request;
- request-user-input prompt;
- plan state update;
- context snapshot update;
- final response;
- failure or cancellation.

The event stream should remain richer than a plain text stream. Plain text can
be derived from it, but protocol adapters should not be the only owner of
structured output.

Runtime subscribers receive `RuntimeControlEvent` objects, not raw TUI text.
Each event carries:

- a stable event id;
- a monotonically increasing sequence number from the shared runtime bus;
- source provenance;
- the structured `RuntimeEvent` payload derived from the internal `AgentEvent`.

The local TUI may still consume raw `AgentEvent` values for compatibility and
presentation-specific state, but ACP, Wire, appserver, and future protocol
adapters should use the structured subscription surface. Slow subscribers must
treat missed broadcast events as a reconnect/resync boundary rather than
silently continuing with a partial stream.

### Prompt Source Registration

External applications may register prompt sources through structured objects:

- source id;
- scope;
- priority or layer;
- budget hint;
- turn-based time-to-live or session lifetime;
- content;
- provenance.

Registered prompt sources enter normal prompt assembly and `/context`. They must
not bypass `PromptRuntime` or introduce unstable top-level prompt prefixes.
Prompt-source time-to-live is turn-based: a transient source may be active for
the next N assembled model turns, or for the lifetime of the session. Wall-clock
expiry is only suitable for adapter leases or connection handles and must not
silently remove prompt context in the middle of a model turn. Persistent sources
must be updated or unregistered explicitly.

### Skill Source Registration

External applications may register:

- skill roots;
- single skill definitions;
- disabled or shadowed skill metadata;
- source precedence hints.

These registrations flow into `SkillRegistry` and `SkillTool`. They must expose
override and parse status through the same observability contract as local
skills.

Source precedence hints are advisory inside the protocol-registered source
layer. They must not override the root-based local precedence defined in
`repo-extension-surface.md` for home, repository, and current-working-directory
sources. A protocol source may only outrank a local source when the user grants
that adapter an explicit higher-trust policy. Conflicts between protocol sources
should be resolved by declared priority first, then registration order, then a
stable source-id tie breaker. Conflict and shadowing decisions must preserve
provenance for `/context` and protocol observability.

### Memory Control

External applications may:

- add workspace or thread memory records;
- query memory metadata;
- request a memory-selection snapshot;
- mark memory records as available, selected, or ineligible through structured
  metadata only when the runtime permits that control.

They may not directly edit the final prompt. Memory must enter the model
working set through `MemorySelection` and context assembly.

### Hook Declaration

External applications may declare hooks for lifecycle points such as:

- `SessionStart`;
- `UserPromptSubmit`;
- `PreToolUse`;
- `PostToolUse`;
- `Stop`;
- `PreCompact`.

The first implementation should record and display hook declarations without
executing them. Later executable hooks must still go through RARA-owned
permission, sandbox, and failure handling.

Hooks that affect prompt context must follow the context-architecture injection
contract. A hook may declare or produce context candidates, cache invalidations,
post-tool deltas, or compaction retain hints, but it must not concatenate raw
text into the final prompt. Final injection happens only during model request
assembly after normal context selection, ordering, deduplication, budget checks,
and `/context` visibility have been applied.

Lifecycle timing is part of the safety boundary:

- `SessionStart` context that must affect the first request has to finish before
  that first request is assembled;
- `UserPromptSubmit` may attach metadata or candidates, but must not rewrite the
  original user text;
- `PostToolUse` output is next-turn context unless the runtime explicitly
  starts a new assembly step;
- `PreCompact` output is advisory retain metadata for the compaction lifecycle;
- `Stop` hooks must be bounded and should not run on prompt-too-long or API
  error paths where hook retry could create a loop.

### Approval and Permission Bridge

External applications may display or relay approval decisions, but RARA remains
the authority for:

- static deny rules;
- sandbox policy;
- prefix allow rules;
- plan approval state;
- command approval state.

Protocol adapters can submit a user decision back to RARA. They cannot directly
mark a tool call as allowed without the runtime recording the decision.

## Control Events

The control plane should emit a single structured event stream that can feed:

- TUI transcript cells;
- ACP prompt responses;
- Wire output;
- `/status`;
- `/context`;
- future headless evaluation traces.

Events should be stable enough to record in thread history or derived runtime
snapshots when they affect recovery.

Minimum event families:

- `SessionEvent`;
- `InputEvent`;
- `AssistantEvent`;
- `ToolEvent`;
- `ApprovalEvent`;
- `PlanEvent`;
- `PromptSourceEvent`;
- `SkillEvent`;
- `MemoryEvent`;
- `HookEvent`;
- `ContextEvent`;
- `WarningEvent`;
- `ErrorEvent`.

`WarningEvent` carries non-fatal runtime issues that should be visible to
protocol adapters without aborting the current request, such as skipped prompt
sources, bootstrap warnings, degraded provider metadata, cache-read misses, or
ignored unsupported adapter options.

## Domain Contracts

### Prompt Runtime

Third-party prompt input must be represented as prompt source objects with
provenance, layer, and budget metadata. The prompt runtime decides whether and
where the source is injected.

### Skills

Third-party skills are local instruction content, not executable authority.
They participate in normal skill precedence and cannot override built-in tools,
sandbox policy, approval rules, or slash commands.

### Memory

Third-party memory writes create `MemoryRecord`-style data. They may contribute
to the current turn only through `MemorySelection`.

### Hooks

Hook declarations are metadata until the hook runtime explicitly supports an
event and handler kind. Hook execution cannot bypass RARA policy.

### Output

Output control is subscription and rendering control, not model control.
External applications can choose how to display events, but the runtime event
stream remains the source of truth.

## ACP and Wire Integration

ACP and Wire should be adapters over the same control plane.

The first ACP milestone should:

- initialize a RARA session;
- submit prompts through input control;
- stream structured output events or map them to ACP response chunks;
- route cancel requests to runtime cancellation;
- expose final stop reasons from the runtime.

The first Wire milestone should reuse the same request and event families. Wire
may define different transport framing, but it should not define a separate
skills, memory, prompt, or hook runtime model.

Wire compatibility is an adapter responsibility, not the native runtime-control
serialization format. A Wire adapter should use the Wire transport framing:

- JSON-RPC 2.0 messages over stdin/stdout, one JSON value per line;
- client-to-agent methods such as `initialize`, `prompt`, `steer`,
  `set_plan_mode`, and `cancel`;
- agent-to-client `event` notifications with `params: { type, payload }`;
- agent-to-client `request` calls with `params: { type, payload }` for
  approvals, questions, tool calls, and hook requests.

Runtime-control events should be translated into Wire message names at the
adapter boundary. For example, internal assistant/tool/status events may become
Wire `ContentPart`, `ToolCall`, `ToolResult`, `StatusUpdate`, or
`ApprovalRequest` messages. The Wire-facing names should follow the external
Wire contract, while runtime-control names remain protocol-neutral and stable
inside RARA.

Wire `steer` maps to runtime follow-up submission during an active turn. The
runtime must still decide when the input is safely consumed. The Wire adapter
should emit the corresponding consumed-input event only after the follow-up has
actually been appended to the next model step, not when the client request is
merely accepted.

## Observability

`/context` should show:

- local and protocol prompt sources;
- active skills and skill roots;
- overridden or failed skill loads;
- memory selected, available, and dropped groups;
- protocol-registered memory records or candidates;
- declared hooks and their support status;
- current controller or protocol source when available.

`/status` should show:

- active protocol adapters;
- current session id;
- provider/model state;
- sandbox/network state;
- pending approvals or questions;
- current plan state;
- active output subscribers when useful.

Both views should read from the same runtime snapshots exposed to protocol
adapters.

## Contracts

- Protocol adapters do not own runtime semantics.
- Runtime domains do not depend on one protocol crate.
- Prompt-affecting inputs must use structured source objects.
- Permission and sandbox policy cannot be bypassed by third-party control.
- Control events are the shared output surface for TUI, ACP, Wire, and headless
  evaluation.
- Features that add user-visible state must define whether that state is
  persisted, compacted, or transient.

## Validation Matrix

- Unit tests for request routing from adapter-neutral inputs to runtime domains.
- Unit tests for provenance propagation on prompt, skill, memory, and hook
  sources.
- Tests proving protocol prompt sources appear in `/context` without changing
  stable prompt-prefix order.
- Tests proving protocol skill sources follow normal precedence and override
  reporting.
- Tests proving memory writes cannot bypass `MemorySelection`.
- ACP adapter tests for prompt, cancellation, and output events once the adapter
  is wired.
- Wire adapter tests should reuse the same control-plane fixtures.

## Open Risks

- ACP and Wire may have different streaming and session semantics. The control
  plane must avoid leaking one protocol's lifecycle into core runtime state.
- Prompt-source registration can become a prompt-injection risk if provenance
  and trust boundaries are not visible.
- Hook execution is high-risk and should remain declaration-only until the
  permission model is explicit.
- Persisting protocol-controlled state requires careful session ownership and
  cleanup rules.

## Source Journals

- [2026-05-01-skilltool-and-verify-specs](../journal/2026-05-01-skilltool-and-verify-specs.md)
- [2026-05-01-memory-selection-spec](../journal/2026-05-01-memory-selection-spec.md)
- [2026-05-01-tui-modular-queued-follow-up](../journal/2026-05-01-tui-modular-queued-follow-up.md)
