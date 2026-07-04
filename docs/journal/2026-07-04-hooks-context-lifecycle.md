# 2026-07-04 Hooks Context Lifecycle

## Summary

- Wired `MemoryQuery` hook dispatch from the `search_memory` tool.
- Mirrored drained hook output into volatile `/context` candidates while keeping
  direct system-message injection before the model turn.
- Added a canonical hooks/plugin lifecycle spec.

## Background

Hook output already had a runtime buffer and direct system-message injection
path, but `/context` still described hook output as non-injected. The
`SearchMemoryTool` also had a TODO for MemoryQuery hooks, so memory queries did
not reach hook callbacks even when the lifecycle existed in the control-plane
types.

## Key Decisions

- `MemoryQuery` is dispatched through an explicit `HookRuntime` method because
  it does not correspond to a normal `AgentEvent` emitted by the agent loop.
- Hook output candidates are marked non-selectable. The model already receives
  the output directly as system context, so retrieval selection must not inject
  it a second time.
- Hook output candidates are session-scoped volatile records with
  `source_type=hook_output`.

## Validation

- `cargo test --locked hook_runtime::tests::dispatch_memory_query_invokes_memory_query_hooks`
- `cargo test --locked hook_output_candidate_is_observable_but_not_reselected`
- `cargo check --locked --workspace --all-targets`
- `cargo clippy --locked --workspace --all-targets -- -D warnings`
