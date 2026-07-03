# Agent Definition Runtime Cache

## Summary

RARA now loads Claude-compatible agent definitions into an
`AgentDefinitionCache` when the runtime is constructed. `spawn_agent` resolves
custom agent definitions from that cache, and `/status` derives its imported
agent summary from the same cached load records.

## Background

The previous implementation could scan `.rara/agents` and `.claude/agents`
from multiple call sites. That kept execution and status display consistent
after the parser unification work, but it still left filesystem discovery on
the hot `spawn_agent` path.

Claude Code keeps agent definitions behind a cache and refreshes that cache
from broader runtime/plugin refresh boundaries. RARA follows that shape:
editing an agent definition does not mutate the running session immediately.
The next runtime rebuild constructs a new cache snapshot.

## Key Decisions

- Keep `.rara/agents` as the preferred workspace path and `.claude/agents` as
  the compatibility path.
- Construct `AgentDefinitionCache` during runtime bootstrap and share it with
  the `AgentTool` and the top-level `Agent`.
- Resolve `spawn_agent` definitions from the cached registry instead of
  rescanning the filesystem per spawn.
- Project `/status` imported-agent lines from cached load records so execution
  and display cannot drift.
- Do not add a dedicated `/reload-agents` slash command. Runtime rebuild is the
  refresh boundary for now, matching the broader Claude Code pattern rather
  than creating a RARA-only command surface.

## Validation

```bash
cargo test agent_definition_cache_refreshes_on_new_runtime_cache -- --nocapture
cargo test spawn_agent_definition_affects_prompt_tools_max_turns_and_plan_mode -- --nocapture
cargo test agents_ext::tests -- --nocapture
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets -- -D warnings
```

## Follow-Ups

- Implement the remaining Claude-compatible `AgentDefinition` metadata:
  `token_budget`, `permission_mode`, `hidden`, and description/listing
  behavior.
