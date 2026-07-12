# 2026-07-11 · Harbor ATIF trajectory

## Summary

RARA's Harbor adapter now declares ATIF support and writes
`agent/trajectory.json` for Terminal-Bench runs. The adapter still preserves
the raw `rara exec --json` stream as `/logs/agent/rara-exec.jsonl`, but Harbor
can now upload and view a normalized trajectory artifact.

## Scope

- Converts RARA `thread.started`, `turn.completed`, `turn.failed`, and
  `item.completed` JSONL events into Harbor's ATIF `Trajectory` model.
- Records the benchmark instruction as the user step.
- Emits assistant messages, reasoning, tool calls, tool progress, tool
  results, system status, and failure messages as trajectory steps or
  observations.
- Links tool observations back to the RARA item id for the originating tool
  call when possible.
- Propagates final token counts from the RARA turn completion event into
  Harbor agent context metrics.

## Key Decisions

- The adapter owns ATIF conversion because Harbor owns the artifact contract,
  while `rara exec` remains responsible for the stable RARA JSONL event stream.
- Tool result events do not currently carry an explicit call id. Because RARA
  executes tool calls in emission order, the adapter associates same-name
  progress and results with the earliest unmatched call, then appends the
  observation to that call's ATIF step. This keeps `source_call_id` valid for
  multiple same-name calls in one model response. The raw JSONL remains
  available for debugging if an external event producer breaks that ordering.
- The trajectory uses Harbor's current `ATIF-v1.7` model instead of a
  RARA-local JSON schema.

## Validation

```bash
HARBOR_SITE_PACKAGES=$(find /Users/hawkingrei/.local/share/uv/tools/harbor/lib -path '*/site-packages' -type d | head -1)
PYTHONPATH="${HARBOR_SITE_PACKAGES}:tools/harbor:." python -m unittest tools.harbor.test_rara_agent
cargo fmt
git diff --check
```

## Follow-Ups

- A future `rara exec` event revision can include explicit tool call ids on
  tool result and progress events to remove the adapter-side ordering
  association.
