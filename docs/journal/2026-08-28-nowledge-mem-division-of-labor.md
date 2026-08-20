# Nowledge Mem Division of Labor

## What

Clarified the division of labor between RARA's local memory and Nowledge Mem
at the model-facing write surfaces, so the same durable fact is no longer
distilled into both stores.

## Why

Both a local write surface (`update_project_memory` → `.rara/memory.md`) and
the Nowledge Mem `distill-memory` skill exist for "remember something durable".
The spec already stated the split (local = short-term file-backed substrate,
official Mem = cross-tool semantic knowledge), but the tool/skill descriptions
did not carry that routing rule, so the model had no call-time signal about
which store to write.

## What changed

- `src/tools/workspace.rs`: `update_project_memory` now states it is the
  workspace-local `.rara/memory.md` surface and points cross-tool/cross-workspace
  knowledge at the Nowledge Mem `distill-memory` skill.
- `src/plugin_middleware/builtin.rs`: the `distill-memory` skill description and
  body now mark Nowledge Mem as the durable/cross-tool authority and tell the
  model not to persist the same fact into local memory.
- `docs/features/memory-records.md`: added a `Division of Labor` contract section.

## Trade-offs

This is a prompt/tool-description + spec change rather than a runtime code
change. It does not remove the local `distill_thread_memories` path, which
remains consistent with the spec (local records are the workspace-local
file-backed substrate).

## Remains

- Runtime-side automatic read: pre-fetching the Nowledge Mem Context Bundle /
  Working Memory into assembled context is still model-driven, not runtime-owned.
