# 2026-05-09 MCP Resource Context References

## Context

The memory/context backlog called out MCP resources as future context sources.
The existing retrieval path already normalized memory, thread, vector, and
file-search candidates through `RetrievalCandidate`, but MCP resource references
had no typed boundary and therefore could not appear in `/context`.

## Change

- Added `McpResourceReference` and `mcp_resource_candidate` as the MCP resource
  adapter into the retrieval candidate model.
- Added a precomputed MCP resource provider to the retrieval provider sequence.
- Extended `RuntimeContextInputs` and `Agent` state with
  `mcp_resource_candidates` so protocol/MCP adapters can supply references
  without mutating prompt text.
- Added an `mcp_resource` source entry to `/context` retrieval source status.
- Kept MCP resource references non-selectable until a resource body loader and
  excerpt-selection policy exist.

## Boundaries

- This does not start MCP servers, list live resources, or fetch resource
  bodies.
- This does not inject MCP resource content into the model request.
- The reference lives in the volatile retrieval/context suffix, preserving
  stable prompt-prefix ordering.

## Validation

- `cargo fmt --check`
- `cargo test retrieval_sources_include_mcp_resource_references --locked`
- `cargo test provider_boundary_collects_current_sources_in_stable_order --locked`

The targeted tests passed. The commands still print historical repository
warnings unrelated to this slice.
