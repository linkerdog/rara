# Portable Tool Contract Checkpoint

## Summary

Move the existing tool contract and registry into `rara-core`, preserving the
old `rara_tools::tool` API through direct re-exports. Native implementations and
their dependencies remain in `rara-tools`. Add a fresh downstream Git consumer
and CI gate for the LLM/tool contracts together.

## Background And Decisions

Issues #860 and #871 share a dependency obstacle: even injected host tools
previously required a crate containing native implementations and memory.
`RuntimeSessionBuilder::build` also still enters application bootstrap and its
agent/client graph. Moving a facade alone cannot solve full runtime packaging.

The reference review used local Codex revision
`f959e7fc9832dfa0ebfb6542ab1bbf829638ac24` (`tools/src/tool_executor.rs`,
`core/src/tools/registry.rs`, and `core/src/tools/context.rs`): the shared
executor contract is separate from core-owned hooks, telemetry, and invocation
state. Claude Code revision `4b9d30f7953273e567a18eb819f4eddd45fcc877`
(`src/Tool.ts`) keeps cancellation and execution context separate from model
arguments. This change adapts those boundaries without adding their policies
or changing existing dispatch behavior.

The patch-specific `From<PatchError> for ToolError` would violate the orphan
rules after moving `ToolError`. Its three call sites now use an explicit adapter
conversion with the same category and message. This removes that implicit
conversion API; it avoids coupling either the portable core or the standalone
patch parser to the native tool adapter. Other contract types and signatures
remain the same through re-exports. Direct observability/thiserror dependencies
are removed from the implementation crate once its contract no longer owns them.

The selected order is shared contracts, shared application/host executor,
minimal session package, then browser effects and execution. Entry/exit gates
and the single-loop requirement are recorded in the
[canonical contract](../features/portable-tool-contracts.md). A second host-only
agent loop, copied workspace patches, and a path-only downstream acceptance
fixture would not satisfy the issues.

## Validation

- Baseline external Git consumer at
  `cf9c2d268db25e792aa0fd22bb0512d72fbb63a2` resolves a fresh lockfile and fails
  specifically at the missing `rara_core::tool` import.
- `cargo test --locked -p rara-core -p rara-tools`: 9 core and 40 implementation
  tests, including compatibility trait/registry identity, default dispatch,
  and patch parsing/context/execution error categories.
- `cargo test --locked --test runtime_session --test embedded_runtime`: 9 host
  integration tests, including identity, cancellation, replay, and concurrency.
- `cargo check --locked --target wasm32-unknown-unknown -p rara-core`, strict
  workspace/all-target Clippy, and Cargo formatting.
- Default `bazel test //crates/rara-core:rara_core_tests
  //crates/rara-tools:rara_tools_tests`: both targets pass after restoring missing
  external repositories through targeted fetches. No Bazel configuration changes;
  the generated lock records the moved dependency edges.
- Final remote revision check: `scripts/check_downstream_core.py --rev <sha>`
  creates a temporary workspace, audits the core dependency closure, tests fake
  backend/custom-tool contracts, and compiles those tests for the browser target.
  Record the published revision and outcome in the PR validation evidence.

## Follow-Ups

This checkpoint does not expose a lightweight `RuntimeSession`. Extract the
existing execution machinery and its application policy seams before moving
session ownership; extend the external fixture to the real session API before
closing #860. Serializable transitions, browser transport/runtime tests,
accounting clocks, future bounds, provider crates, resolver migration, and the
remaining legacy lint allowances remain #871 work. See [TODO](../todo.md).
