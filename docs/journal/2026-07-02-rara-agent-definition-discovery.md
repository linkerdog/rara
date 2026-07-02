# RARA Agent Definition Discovery

## Summary

RARA now treats `.rara/agents/*.md` as the canonical project-local location for
custom subagent definitions while keeping `.claude/agents/*.md` as a legacy
compatibility import path. Both locations use the Claude-compatible YAML
frontmatter plus markdown body format.

## Background

Agent execution and `/status` discovery previously parsed imported agent files
through separate code paths. Execution used the `AgentDefinition` frontmatter
parser, while status discovery inferred labels and descriptions from markdown
headings. That made runtime behavior and explainability able to drift.

## Key Decisions

- Use one Claude-compatible parser for both execution and status projection.
- Load lower-precedence roots first, then let later roots override same-name
  definitions.
- Keep `.claude/agents` as compatibility input, but make `.rara/agents` the
  preferred RARA path and highest-precedence workspace root.
- Keep `/status` scoped to workspace extension files; execution continues to
  support home and workspace roots.

## Validation

```bash
cargo test agents_ext::tests -- --nocapture
cargo test tools::agent_test::resolve_spawn_agent_definition_loads_workspace_agent -- --nocapture
cargo test tools::agent_test::rara_agent_definition_overrides_legacy_claude_definition -- --nocapture
cargo test tools::agent_test::spawn_agent_definition_lookup_uses_normalized_label -- --nocapture
cargo test agent_definition_uses_filename_when_frontmatter_omits_name -- --nocapture
cargo test agent_definition_accepts_empty_frontmatter -- --nocapture
cargo test agent_home_dir_falls_back_to_userprofile -- --nocapture
cargo test discover_repo_agents_uses_frontmatter_name_as_id -- --nocapture
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets -- -D warnings
```

## Follow-Ups

- Cache agent definitions at runtime construction time and refresh them through
  runtime rebuild.
- Add end-to-end `spawn_agent` coverage for prompt body, tool filtering,
  `maxTurns`, and `planModeRequired`.
