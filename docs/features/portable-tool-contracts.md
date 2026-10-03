# Portable Tool Contracts

## Problem

An embedding host implementing a custom tool should not depend on built-in file,
memory, or process tools. The former `rara-tools` contract module pulled those
implementations and Tokio into every consumer's dependency graph. This also
blocked extraction of the shared agent execution layer.

## Scope

- `rara_core::tool` owns `Tool`, `ToolError`, `ToolManager`, `ToolCallContext`,
  `ToolProgressEvent`, and `ToolOutputStream`.
- The existing `rara_tools::tool` path re-exports exactly those definitions.
- A downstream fixture builds the core package from an explicit Git revision,
  outside the repository workspace, without copied patches or a lockfile.

## Non-Goals

This boundary does not extract `RuntimeSession` or introduce another agent loop.
The application still owns provider construction, bootstrap, agent execution,
and the session actor. Full downstream runtime support remains
[#860](https://github.com/linkerdog/rara/issues/860); browser execution and the
serializable sans-IO agent remain
[#871](https://github.com/linkerdog/rara/issues/871).

## Architecture

The dependency direction is `rara-tools -> rara-core -> rara-observability`.
Core contains the registry and invocation contract; native implementations stay
in `rara-tools` and the application. Existing application call sites consume
the same trait and registry through re-exports, with no conversion layer.

The core's production dependency closure is limited to error, async-trait,
serialization, procedural macro support, and inference context types. It has
no Tokio, transport, filesystem implementation, provider, TUI, ACP, OAuth,
database, or local model dependency. `PathBuf` remains an inert context value;
it does not make filesystem access portable to a browser.

## Contracts

- Tool schemas and registry iteration remain ordered by tool name. Registering
  the same name replaces its implementation; filtering and retention preserve
  the remaining order and schema shape.
- The context-aware default calls the event-aware method, whose default calls
  `call`. Existing tools that override any one of these methods retain dispatch.
- Context is supplied by the caller, separately from model JSON. Session, turn,
  provider call ID, workspace, cancellation, and inference context remain intact.
  Tool implementations own authorization and must not trust model-supplied
  replacements for those values.
- Cancellation is cooperative. The registry does not interrupt a tool or
  invent a cancelled result; a context-aware tool must observe its token.
- Error variants and progress values retain their existing meanings. The
  compatibility path is a re-export, not a second trait or wrapper.
- The patch adapter maps `PatchError` explicitly, preserving category and
  message. Its former `From<PatchError> for ToolError` cannot survive Rust's
  orphan rules after extraction. Downstream code using that implicit conversion
  must map the two patch error variants explicitly; neither portable core nor
  the independent patch parser takes a reverse adapter dependency.
- Native `Send + Sync` and `async_trait` future bounds are unchanged. Browser
  target compilation alone does not promise compatibility with non-Send fetch
  futures or working inference clocks.

## Validation Matrix

| Boundary | Check |
|---|---|
| Registry | Core tests preserve schema order, replacement, filtering, and retention |
| Compatibility | A tool implementing the core trait is accepted by the existing tools registry |
| Invocation | Downstream fake backend and custom tool exercise streamed text, call IDs, progress, trusted context, and cooperative cancellation |
| Package resolution | Fresh temporary Cargo project uses `git` plus a full `rev`; no patches or copied lockfile |
| Dependency closure | Downstream metadata checks an explicit core dependency allowlist |
| Browser compilation | Both workspace core and downstream fixture check `wasm32-unknown-unknown` without feature flags |
| Integration | Core/tools tests, workspace Clippy, formatting, and default Bazel crate tests |

## Downstream Use

Use the `rara-core` package from a reviewed full Git commit, with the normal
Cargo dependency form below. Replace the revision placeholder with that commit.
There is no crates.io release or semver compatibility guarantee asserted by
this extraction; update the revision deliberately and run host conformance
tests. Do not depend on the application package for these contracts.

```toml
[dependencies]
rara-core = { git = "https://github.com/linkerdog/rara.git", rev = "<full-commit-sha>" }
```

`scripts/check_downstream_core.py --rev <full-commit-sha>` checks this mechanism
against the remote repository. Its temporary project owns its fresh lockfile
and does not inherit repository Cargo configuration. CI uses the actual PR head
repository and revision, including for forks.

## Runtime Extraction Sequence

1. **Shared contracts:** establish one LLM/tool type identity and a standalone
   Git dependency check. Exit when native tests, graph audit, and browser
   compilation pass. This is the scope of this checkpoint.
2. **Shared execution:** separate application hooks, memory, persistence,
   extension discovery, and provider factories from the existing execution
   machinery. Keep their effects behind explicit host adapters and make the
   application consume the extracted implementation. Enter after shared
   contracts are stable; exit after existing planning, approval, cancellation,
   tool identity, and transcript tests exercise that implementation. A separate
   simplified host loop is not an acceptable intermediate runtime API.
   The [portable loop machine](portable-agent-loop.md) now owns deterministic
   transitions, and its shared asynchronous executor is consumed by the
   application through `LoopEffects`. Model dispatch and response collection are
   also shared. Serial tool admission/invocation/result collection now uses the
   same executor with native policy adapters. Portable context preparation,
   native policy assembly, and full session packaging remain to be extracted.
3. **Session ownership:** move actor, commands, replay, and turn outcomes into a
   minimal runtime package using the shared executor. Preserve the lifecycle
   invariants in [runtime-session.md](runtime-session.md). Exit only when a
   downstream Git fixture injects a fake backend and custom tool through
   `RuntimeSession`, verifies deltas, identities, cancellation and transcript
   readback, and excludes native application integrations from its graph.
4. **Browser execution:** use the serializable transition boundary, extract
   portable effect drivers, and adapt clocks/future bounds and HTTP/SSE
   transports. Browser runtime tests are a distinct gate from compilation;
   control-state serialization alone is not durable session recovery.

Each extraction is independently reviewable. The principal risk is losing
application behavior while reducing dependencies; existing consumers must use
the extracted implementation before its host API is considered delivered.

## Source Journals

- [2026-10-04-portable-tool-contracts](../journal/2026-10-04-portable-tool-contracts.md)
