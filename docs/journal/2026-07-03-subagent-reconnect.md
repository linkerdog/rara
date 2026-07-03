# Subagent Restart Reconnect

## Summary

RARA can now reconnect `subagent_resume` and `subagent_list` to completed
subagent results persisted in the current parent thread after the runtime
restarts.

## Scope

- `subagent_resume` first checks the live in-process background store.
- If the live store does not know the `agent_id`, it reads the current parent
  thread's durable spawn-agent rollout edges and returns the completed result
  metadata when present.
- `subagent_list` merges live in-process records with current-thread durable
  completed spawn-agent edges.
- Sidechain transcript contents remain out of parent model context.
- No new tool is added; the existing subagent control tools gain reconnect
  behavior.

## Key Decisions

- Reconnect uses parent-scoped rollout events instead of scanning every
  sidechain transcript path.
- Persisted reconnect records are marked with `kind = "reconnected"` because
  the original subagent kind is not part of the durable spawn edge.
- In-flight execution remains process-local. If the RARA process exits while a
  background subagent is still running, this change does not restart or continue
  that task.

## Validation

```bash
cargo test subagent_resume_reconnects_completed_sidechain_after_store_restart -- --nocapture
cargo test background_subagent_resume_returns_completed_summary_without_inline_sidechain -- --nocapture
cargo test background_plan_agent_resume_returns_plan_state -- --nocapture
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets -- -D warnings
git diff --check
```

## Follow-Ups

- A durable task registry would be required before RARA can restart or continue
  still-running subagents after process exit.
