# Built-In Specialist Agents

## Summary

Added three reusable read-only subagent profiles: `code-reviewer`, `architect`,
and `researcher`. The existing `plan` profile remains the planning specialist.

## Background

The runtime already accepted Claude-compatible user and workspace agent
definitions, but common specialist names required a local definition and were
not described by the `spawn_agent` tool contract. Claude Code and Codex both
use small built-in role registries with explicit purpose and capability
descriptions, so this checkpoint adopts that pattern with role-specific child
authority.

The prompt comparison used the local Claude Code implementation and the
`claude-code-system-prompts` extraction repository. The latter is reference
material extracted from compiled packages, not Anthropic-maintained source. Its
`Explore` prompt is repository-search-specific, while the managed `Web
researcher` example enables `read`, `glob`, `grep`, `web_fetch`, and
`web_search`, and requires a source URL or file path for each claim.

## Key Decisions

- Keep `code-reviewer` and `architect` repository-only with `Read`, `Glob`, and
  `Grep`.
- Give `researcher` the same repository tools plus `WebSearch` and `WebFetch`;
  require source URLs or repository paths, prefer primary sources, and treat
  search results as leads rather than proof.
- Register web tools in the child-owned custom tool manager, then rely on the
  existing per-definition whitelist so no other specialist is widened.
- Inherit the parent provider and model instead of pinning a model.
- Keep `plan_agent` as the dedicated implementation-planning surface instead
  of adding a `planner` alias.
- Allow user and workspace definitions to override a built-in specialist with
  the existing definition precedence.
- Expose the built-in role purposes in the `spawn_agent` tool description.
- Defer description-driven automatic delegation and `team_create` custom-role
  routing.

## Validation

```bash
cargo test builtin_specialist -- --nocapture
cargo test resolve_spawn_agent_definition -- --nocapture
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets -- -D warnings
git diff --check
```

## Follow-Ups

- Evaluate a structured agent catalog in the runtime prompt before adding
  automatic role selection.
- Decide separately whether `team_create` should accept named agent
  definitions in addition to its built-in task kinds.
