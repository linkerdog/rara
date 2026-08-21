# Multi-Agent Orchestration And Embedded Runtime

## What changed

RARA now owns live subagents through one session-tree-scoped
`AgentTreeControl` instead of a process-global background registry. Foreground,
background, and team launches share one semaphore-backed active-child budget.
The default permits three children in addition to the root, while embedded
applications may supply a non-zero capacity through `AgentTreeConfig`.

Background completion, interruption, and parent-to-child messages use bounded
FIFO mailboxes. Completion is emitted once, targeted waits ignore unrelated
activity, and each `Agent` injects drained messages at the next model boundary
after existing history before checkpointing the request context.

All launch surfaces accept invocation-level `provider` and `model` routing.
Named agent definitions remain the owner of prompts and permissions; routing
uses invocation overrides first, profile defaults second, and the parent
backend last. A bare model override retains the profile provider, while an
explicit provider selects that provider's configured/default model.

The root package now builds as both a library and the existing binary.
`EmbeddedRuntime` constructs the same workspace runtime without changing the
process current directory, accepts an isolated state root, reports typed agent
events, and exposes session-authorized list, wait, message, follow-up, and
interrupt operations. The binary is a thin adapter over `rara::run_cli()`.
The state root also flows into root and per-child backend construction so
provider credential storage cannot fall back to the host application's default
RARA home.

The root Bazel graph mirrors the Cargo package boundary: `rara_lib` owns the
library sources, the `rara` binary depends on that library, and library unit
tests plus the embedded integration test are independently addressable test
targets.

The implementation also completes real source splits required by the project
size contract. `agent.rs` now keeps shared types and helpers while runtime
entrypoints and the execution loop live in separate modules; configuration
keeps `RaraConfig` and its tests in focused submodules; TUI command tests and
maintenance helpers were extracted without using `include!`. Every touched
Rust source file remains below 1000 lines.

## Reference patterns

The implementation adapts current upstream structures rather than copying
their APIs:

- Codex keeps typed agent paths, a session-owned registry, shared concurrency
  guidance, input queues, and explicit spawn/wait/message controls. Its current
  multi-agent mode is prompt/world-state policy; higher reasoning effort is not
  treated as implicit mutation authority.
- OpenCode separates an agent profile from its child session. The profile owns
  prompt, permissions, and an optional default model; task execution creates a
  child session and otherwise inherits the parent model. RARA preserves that
  separation and adds an explicit invocation override for heterogeneous teams.
- Claude Code's public subagent contract uses fresh child context, separates
  tool/permission policy from model selection, and resolves an invocation model
  before the agent definition and parent model. RARA mirrors those applicable
  boundaries without adopting Claude-specific environment overrides.

Source checkpoints:

- Codex `2151d3a5b78ca93128496b26333bc30187385a5f`: [session multi-agent
  policy](https://github.com/openai/codex/blob/2151d3a5b78ca93128496b26333bc30187385a5f/codex-rs/core/src/session/multi_agents.rs),
  agent registry/control, and input queue modules.
- OpenCode `e11dbd02068aa36723dd43da43c247ade82d2fe7`: [task
  tool](https://github.com/anomalyco/opencode/blob/e11dbd02068aa36723dd43da43c247ade82d2fe7/packages/opencode/src/tool/task.ts),
  [agent profiles](https://github.com/anomalyco/opencode/blob/e11dbd02068aa36723dd43da43c247ade82d2fe7/packages/opencode/src/agent/agent.ts),
  and agent documentation.
- [Claude Code subagent documentation](https://code.claude.com/docs/en/sub-agents)
  consulted on 2026-08-21.

## Design decisions

### Ownership and lifecycle

One runtime bootstrap creates one control and gives the same handle to its root
agent and orchestration tools. Every control operation checks the caller's
parent session. Stable paths are useful identity, but path text never replaces
session authorization.

Backend rebuilds pass the current control into bootstrap before tools are
assembled. This keeps existing children, rebuilt launch tools, root mailbox
delivery, and external session handles on the same tree across model changes.

Cancellation changes lifecycle state and signals the child immediately, but
the semaphore permit remains held until the execution future exits. This
prevents cancelled provider requests from temporarily exceeding the configured
budget.

### Delivery and context

Foreground and team results remain paired tool results. Only asynchronous
completion enters the parent mailbox, which prevents duplicate delivery. The
mailbox is a volatile suffix source: it does not alter stable system-prompt
sections and it does not copy the complete parent transcript into children.
Gemini request construction now concatenates ordered system segments so a
mailbox suffix cannot replace the stable root instructions.

The first rollout keeps children non-recursive. Parent-history forks remain
disabled until a projector can preserve tool-use/result pairs and enforce an
explicit inherited-context budget.

### Policy and models

`MultiAgentPolicy` is `disabled`, `explicit`, or `proactive_read_only`.
`explicit` remains the default. Proactive guidance is limited to bounded
read-only exploration, planning, research, architecture, and review roles;
configured child permissions remain authoritative regardless of model choice.

Provider/model resolution is recorded on the live child after backend
construction, so running and completed snapshots converge on the backend that
actually receives the request rather than retaining only the requested alias.

### Library boundary

The initial public API is deliberately narrow. It exports configuration,
typed query events, the embedded facade, and bounded agent-control value types;
TUI state and the raw mutable `Agent` remain private. This provides an
embeddable path now without declaring the current internal module graph stable.

## Trade-offs

- Messages are observed between model requests, not while a provider request
  is in flight.
- Mailboxes and running tasks are process-local; durable sidechain records do
  not resume an in-flight child after process restart.
- The root library still compiles modules that should eventually move into
  focused runtime crates.
- Runtime construction still uses configured provider assembly. A future
  dependency-injection builder should accept programmatic backends and stores.

## Verification targets

- `cargo fmt --all -- --check`
- `cargo check --all-targets`
- `cargo test -p rara-config multi_agent_policy`
- `cargo test --lib tools::agent::agent_control::tests`
- `cargo test --lib agent::tests::mailbox`
- `cargo test --lib spawn_agent_invocation_model_keeps_profile_provider`
- `cargo test --lib routes_`
- `cargo test --test embedded_runtime`
- `cargo test --all-targets`
- `cargo clippy --all-targets -- -D warnings`
- `bazel build //:rara //:rara_lib //:rara_unit_tests //:embedded_runtime_tests`
- `bazel test --test_output=errors //:rara_unit_tests //:embedded_runtime_tests`
- `git diff --check`

## Remaining work

Recursive delegation, parent-history projection, proactive-policy evaluation,
restart-durable running agents, dependency injection, and the planned runtime
crate split remain tracked in `docs/todo.md`.
