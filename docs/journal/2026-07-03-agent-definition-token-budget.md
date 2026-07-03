# Agent Definition Token Budget

## Summary

RARA now applies Claude-compatible `AgentDefinition.tokenBudget` values to
spawned subagent execution.

## Scope

This checkpoint covers subagent runtime enforcement:

- `tokenBudget` must be a positive token count that fits in `u32`.
- Budget accounting uses provider-reported model input plus output tokens.
- Cache hit and miss counters remain visible telemetry and are not added to the
  budget total.
- A budgeted subagent that reaches or exceeds its budget stops before starting
  another model turn and returns `status: "budget_limited"`.
- Parent spawn-agent edges persist the configured `tokenBudget`.

## Key Decisions

- Enforce the budget in the shared `Agent` loop rather than only in
  `spawn_agent`, so background, team, and direct subagent entrypoints share the
  same behavior.
- Treat budget exhaustion as a soft stop instead of an error; the subagent can
  still return the work and tool results already produced before the limit was
  reached.
- Reject invalid budgets before subagent creation so malformed frontmatter does
  not start work with an unintended unlimited budget.

## Validation

```bash
cargo test spawn_agent_definition_token_budget_stops_after_budget_exhaustion -- --nocapture
cargo test agent_token_budget_rejects_invalid_values -- --nocapture
cargo test spawn_agent_definition_affects_prompt_tools_max_turns_and_plan_mode -- --nocapture
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets -- -D warnings
git diff --check
```

## Follow-Ups

- No remaining `AgentDefinition` execution metadata follow-up is open after
  `tokenBudget` and `permissionMode`.
