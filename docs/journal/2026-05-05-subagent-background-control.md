# Sub-Agent Background Control

## Summary

This checkpoint adds an in-process background control layer for sub-agents on
top of the existing parent-scoped sidechain transcript contract.

## Behavior

- `spawn_agent`, `explore_agent`, and `plan_agent` accept
  `run_in_background = true`.
- A background sub-agent returns immediately with:
  - `agent_id`;
  - preallocated child `session_id`;
  - `status = running`;
  - parent session metadata when available.
- `subagent_list` lists live in-process background sub-agents.
- `subagent_resume` returns the current status or completed summary for a
  background sub-agent.
- `subagent_stop` marks a running background sub-agent as `cancelled` and sets
  the model cancellation token.

The parent context receives only structured status/result metadata. The child
sidechain transcript remains under:

```text
rollouts/<parent_session_id>/subagents/agent-<agent_id>.jsonl
```

## Boundaries

- This is not a cross-process task registry.
- Restart/reattach after the RARA process exits remains future work.
- `team_create` remains a synchronous aggregation tool.
- Completed background sub-agents still persist the same sidechain transcript
  and spawn-edge event as foreground sub-agents.

## Validation

- `cargo test background_subagent -- --nocapture`
- `cargo test background_plan_agent_resume_returns_plan_state -- --nocapture`
- `cargo check`
