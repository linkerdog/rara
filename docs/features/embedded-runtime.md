# Embedded Runtime

## Problem

RARA's runtime is assembled inside the executable crate and most construction
types are crate-private. ACP, Wire, headless execution, and the TUI already use
the same internal agent loop, but another Rust application cannot construct and
drive that loop through a supported library API.

The executable must become one adapter over a reusable runtime instead of the
owner of runtime behavior.

## Scope

- Build the root package as both a Rust library and the existing `rara` binary.
- Keep `main.rs` as a thin call into the library CLI entrypoint.
- Expose a small `EmbeddedRuntime` facade that initializes one workspace-scoped
  runtime from `RaraConfig`.
- Let an embedding application submit prompts, receive typed `AgentEvent`
  callbacks, inspect the session id, and access the session-scoped
  `AgentTreeControl`.
- Preserve the same plugin, hook, MCP, skill, sandbox, LSP, memory, and tool
  assembly used by CLI surfaces.

## Non-Goals

- Stabilize every internal RARA module as public API.
- Complete the planned `rara-agent`, `rara-control-plane`, and `rara-app` crate
  split in this change.
- Add an FFI, C ABI, WASM agent loop, or network server wrapper.
- Let embedded callers bypass sandbox, permission, or workspace ownership
  checks.
- Make `EmbeddedRuntime` thread-safe for concurrent mutable prompt submission;
  callers own serialization or create separate runtime instances.

## Architecture

### Package Boundary

`src/lib.rs` owns module compilation and exports the supported facade. The
binary imports that library and only performs process-level error rendering and
exit handling. Runtime modules remain private unless they are intentionally
re-exported through the facade.

This is an incremental boundary on the path to the crate split documented in
`crate-split.md`: applications can embed RARA now without declaring the current
internal module graph stable forever.

### Construction

`EmbeddedRuntime::from_config` accepts:

- a borrowed `RaraConfig`;
- an explicit workspace path;
- optional runtime bootstrap options.

Bootstrap options include a state root, plugin directories, and an
`AgentTreeConfig` with a non-zero active-child limit.

The explicit state root is also passed through provider construction and
per-child backend resolution. Provider-owned credential storage therefore
remains inside the embedding application's chosen state scope instead of
falling back to the process user's default RARA home.

Construction must not mutate the process current directory. It creates fresh
session-scoped registries and control handles for every instance. Provider
credentials and backend selection continue to use the supplied configuration.

### Execution And Events

The facade exposes a prompt method with an `FnMut(AgentEvent)` callback. Events
are typed Rust values; JSON or protocol translation remains the embedding
application's boundary. The method accepts `AgentOutputMode`, so library users
can avoid terminal writes.

The facade also exposes identity and control access needed to integrate an
external scheduler or UI:

- current session id;
- the session-scoped `AgentTreeControl` handle and configured capacity;
- typed list, wait, message, follow-up, and interrupt methods that apply the
  embedded root session's ownership boundary;
- runtime event bus subscription when protocol-level fan-out is required.

Mutable access to the raw `Agent` is not part of the initial stable facade.

## Contracts

- Constructing two embedded runtimes creates two isolated agent trees.
- An explicit workspace path is used without changing process cwd.
- An explicit state root scopes runtime configuration, workspace data,
  sessions, and provider-owned credential storage.
- `AgentOutputMode::Silent` produces no direct terminal assistant output.
- Library and CLI construction use the same runtime bootstrap path.
- Typed events are emitted in the same order as direct `Agent` execution.
- Public facade types do not expose TUI state types in method signatures.
- Programmatic agent control cannot address children outside the embedded
  runtime's root session.
- New runtime features must be assembled below this facade so embedded, ACP,
  Wire, headless, and TUI surfaces receive them consistently.

## Validation Matrix

| Area | Validation |
| --- | --- |
| Package | Cargo and Bazel both build the `rara` library and thin binary targets. |
| Workspace | An embedded runtime constructed for a temp workspace retains that path while process cwd is unchanged. |
| State scope | A provider with local auth storage creates it under the explicit state root. |
| Events | A mock-backend query reports typed assistant/model lifecycle events through the callback. |
| Isolation | Two embedded instances expose distinct session ids and agent-tree controls. |
| CLI parity | The binary delegates to the library CLI entrypoint without duplicating module declarations. |
| Bazel parity | `rara_lib`, `rara`, library unit tests, and the embedded integration test are separate Bazel targets. |
| Warning hygiene | Library and binary targets add no compiler or Clippy warnings. |

## Open Risks

- The root library initially compiles modules that will later move into
  domain-specific crates; public exports must stay deliberately narrow.
- Runtime initialization still discovers configured local extensions and may
  perform provider setup; a future dependency-injection builder should support
  fully programmatic backends and stores.
- Callback-based execution is sufficient for Rust embedding but a structured
  command handle will be needed for concurrent app-server use.

## Source Journals

- `2026-08-21-multi-agent-orchestration.md` — first embeddable facade and
  session-scoped multi-agent integration checkpoint.
