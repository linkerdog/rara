# Multi-Agent Orchestration

## Problem

RARA can launch foreground and background subagents through several tools, but
the existing launch paths do not share one ownership or resource model.
Process-global background state can leak live agent metadata across runtime
sessions, `team_create` and background launches enforce different concurrency
limits, and parents must poll individual records to discover completion.

Multi-agent behavior needs a deterministic runtime control plane before the
model is encouraged to delegate proactively. Model policy decides when
delegation is useful; the runtime owns identity, isolation, lifecycle, resource
limits, cancellation, and result delivery.

## Scope

- Replace process-global background-subagent state with a control object owned
  by one runtime session tree.
- Apply one active-subagent budget to foreground, background, and team launch
  paths.
- Keep a typed registry of children, including parent session, child session,
  stable agent id, path, lifecycle state, and bounded result metadata.
- Deliver asynchronous completion exactly once through a parent mailbox.
- Add agent-oriented list, wait, message, follow-up, and interrupt tools while
  retaining the existing `subagent_*` compatibility tools.
- Allow every launch surface to select a provider/model per child while
  preserving agent-profile defaults and parent-model inheritance.
- Inject queued mailbox messages at model-turn boundaries as a volatile suffix
  and persist the injected message in the owning session transcript.
- Define an explicit multi-agent policy that is independent from provider
  reasoning effort.

## Non-Goals

- Infer proactive delegation directly from `reasoning_effort = "ultra"`.
- Give subagents recursive spawn tools in the first rollout.
- Copy an entire parent transcript into a child prompt.
- Resume an in-flight model request after the parent runtime process exits.
- Allow a proactive policy to bypass agent-definition tool or permission
  restrictions.
- Merge child edits automatically or provide isolated child worktrees.

## Architecture

### Runtime Ownership

Each `RuntimeBootstrap` creates one `AgentTreeControl` and gives the same handle
to the root `Agent` and every multi-agent tool in that runtime. The control is
never stored in a process-global strong reference. Separate ACP, Wire,
headless, or TUI sessions therefore cannot enumerate, message, or cancel one
another's live agents.

A backend or model rebuild for an existing session supplies that session's
current control back into runtime bootstrap. Reusing the handle at assembly
time is required: replacing only the root `Agent` field would leave rebuilt
tools attached to a different registry and split mailbox delivery from control
operations.

`AgentTreeControl` is also exposed through the embedded-runtime facade. Its
constructor accepts an `AgentTreeConfig` with a non-zero capacity and its
lifecycle does not depend on CLI or TUI state, so another Rust application can
own several independent trees in one process. `EmbeddedRuntime` wraps list,
wait, message, follow-up, and interrupt operations with its root session id so
callers do not have to reproduce tool-context authorization.

The root session is the tree owner. A child record stores both an opaque
`agent_id` and a display path. Tools accept either identity, but authorization
is always checked against the caller's session id rather than path text.

### Resource Model

One semaphore limits active child executions across all launch surfaces. The
default tree capacity is three active children, leaving the root as the fourth
execution slot. A background launch fails immediately when capacity is full;
foreground and team calls wait for a permit so already-accepted work is not
silently dropped.

Stopping a child marks it cancelled and signals its cancellation token, but its
permit is released only when the child execution actually exits. This prevents
cancelled-but-still-running work from temporarily exceeding the tree budget.

### Mailbox Delivery

Each runtime session id owns a bounded FIFO mailbox. Background completion,
interruption, and explicit inter-agent messages produce typed envelopes with a
monotonic sequence, sender identity, message kind, and payload. Completion is
enqueued once; foreground and team results are returned directly as paired tool
results and are not duplicated in the mailbox.

The owning `Agent` drains its mailbox before a model request. Drained envelopes
are appended as one system message after existing transcript history and are
checkpointed before the request. This keeps the transcript authoritative,
preserves tool-use/tool-result pairing, and avoids changing the stable prompt
prefix. Provider adapters must preserve all ordered system segments; Gemini
combines the stable root instructions and volatile suffix instead of replacing
one with the other. `wait_agent` can wait for mailbox activity and return
matching envelopes as its normal tool result.

### Policy Boundary

Multi-agent policy has three states:

- `disabled`: delegation tools are not exposed;
- `explicit` (default): tools are available, but the model delegates only when
  requested or when the current task contract explicitly calls for parallel
  agent work;
- `proactive_read_only`: the model may independently delegate bounded,
  parallelizable research, review, or planning work to read-only roles.

The policy is not derived from reasoning effort. Model capability and
orchestration policy are independent inputs. `proactive_read_only` does not
authorize mutation: custom/general agents retain their configured permissions
and are not selected proactively.

### Model Routing

Agent identity, permission policy, and model routing are independent. Every
launch resolves a concrete child model with this precedence:

1. per-invocation `provider` / `model` override;
2. the selected agent definition's provider/model;
3. the parent runtime backend.

`model` accepts either a model id for the selected/current provider or an
explicit `provider:model` pair. Supplying both `provider` and a prefixed model
is rejected as ambiguous. The resolved provider and model are captured in the
child record and returned by list, wait, and result surfaces.

This follows OpenCode's separation between an agent profile (prompt,
permissions, default model) and a child session, while adding an invocation
override so one team call can route independent tasks to different models.
It also matches Claude Code's documented per-invocation, agent-definition, then
parent-model order after excluding Claude-specific environment overrides.
Inheritance applies only when neither the call nor the profile selects a model.

### Context Boundary

The initial child context remains task-first. Parent-history fork modes are not
added until a projector can preserve assistant tool-use and tool-result pairs,
apply an explicit token budget, and identify the inherited transcript range.
Inter-agent messages are volatile suffix inputs, not stable prompt sections.
This retains the fresh-context named-subagent boundary documented by Claude
Code while leaving full-context forks for a separately specified feature.

## Contracts

- A runtime session tree cannot observe or control live records owned by a
  different runtime tree.
- Rebuilding a backend within a session preserves the exact control handle used
  by both the root agent and rebuilt orchestration tools.
- Every accepted child execution owns exactly one shared active permit.
- Background launch is fail-fast at capacity; foreground/team launch is
  backpressured.
- A terminal background child produces at most one completion envelope.
- Model resolution is deterministic: invocation override, then agent profile,
  then parent backend. Resolution does not weaken child permissions.
- Cancellation does not release capacity until execution termination.
- Mailbox messages are ordered and removed only when delivered.
- `wait_agent` waits for activity; it does not synthesize child results or
  mutate child lifecycle state.
- Messages sent to a running child become visible at that child's next model
  boundary. A follow-up cannot restart a completed child in this rollout and
  must return an explicit error.
- Existing `subagent_list`, `subagent_resume`, and `subagent_stop` remain
  compatible while enforcing caller-session ownership.
- Stable prompt section order does not change when mailbox content changes.

## Validation Matrix

| Area | Validation |
| --- | --- |
| Session isolation | Two controls and two parent sessions cannot list, resume, message, or stop each other's children. |
| Runtime rebuild | Bootstrap accepts the current control handle and the rebuilt agent retains that exact handle. |
| Shared capacity | Mixed background, foreground, and team launches never hold more than three active permits. |
| Cancellation | A cancelled child keeps its permit until the execution future exits. |
| Completion | Background success, failure, and cancellation each enqueue one terminal envelope. |
| Mailbox order | Multiple messages preserve sequence and are delivered once. |
| Agent context | Drained envelopes appear after existing history, are checkpointed, and do not break tool-result pairing. |
| Provider serialization | Multiple ordered system segments preserve both the stable prompt and mailbox suffix. |
| Compatibility | Existing subagent control tool payloads retain their documented fields. |
| Policy | Default configuration is `explicit`; proactive guidance names only read-only delegation surfaces. |
| Model routing | Mixed-provider team tasks resolve independently and report the backend actually used. |
| Warning hygiene | Focused tests, `cargo check`, and Clippy introduce no warnings in touched code. |

## Open Risks

- A child blocked inside a provider request observes a message only after that
  request returns.
- Process-local mailboxes do not make running children restart-durable.
- Full hierarchical delegation requires child-visible orchestration tools and
  a depth budget; both remain intentionally disabled.
- Parent-history forking needs a pairing-preserving, token-budgeted projection
  before it can be exposed safely.
- Proactive policy quality needs task-level evaluations for unnecessary
  delegation, synthesis quality, latency, and token amplification.

## Source Journals

- `2026-08-21-multi-agent-orchestration.md` — session-tree control, shared
  capacity, mailbox delivery, compatibility tools, and staged policy rollout.
