# 2026-05-09 Retrieval Provider Scope Audit

## Summary

Audited the remaining retrieval-provider TODO after MCP resource references were
implemented.

## Evidence

- `src/context/retrieval_provider.rs` has `PrecomputedMcpResourceProvider`.
- `mcp_resource_candidate(...)` normalizes MCP resource references into
  `RetrievalCandidate`.
- `provider_boundary_collects_current_sources_in_stable_order` asserts MCP
  resource candidates are collected after file-search candidates and remain
  non-selectable until a resource body loader exists.
- `src/context/retrieval_view.rs` exposes MCP resource references in the
  provider view used by `/context`.

## Decision

MCP resource references should no longer be tracked as missing from the
retrieval-provider boundary. The remaining work is hook output candidates and
graph candidates, both of which need source-specific runtime producers before
they can become selectable retrieval inputs.

## Validation

- `rg -n "PrecomputedMcpResourceProvider|mcp_resource_candidate|provider_boundary_collects_current_sources_in_stable_order" src/context`
