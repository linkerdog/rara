# Agent Definition Permission Mode

## Summary

RARA now applies Claude-compatible `AgentDefinition.permissionMode` values to
spawned subagent execution policy.

## Scope

This checkpoint covers runtime policy only:

- `default` and omitted values keep the subagent's normal mode.
- `acceptEdits` keeps execute mode but requires bash approval for mutable shell
  commands.
- `auto` keeps execute mode with the normal auto policy.
- `plan` and `readOnly` force plan mode and read-only subagent tools.
- `bypassPermissions` and `fullAccess` enable full-access approval bypass unless
  `planModeRequired` is set, because plan mode takes precedence.

Values are parsed ASCII case-insensitively, and invalid values fail the
`spawn_agent` request before creating a subagent.

## Key Decisions

- Treat `planModeRequired` as stronger than any permission mode, matching the
  Claude Code pattern where plan mode takes precedence over bypass permissions.
- Force the read-only tool manager for `permissionMode: plan` so shared task
  mutation tools are not available in read-only subagents.
- Keep parser aliases explicit in the error message so users can self-correct
  invalid frontmatter without reading the source.
- Keep `token_budget` as the remaining execution metadata follow-up.

## Validation

```bash
cargo test filtered_tool_manager_permission_mode_plan_forces_read_only_tools -- --nocapture
cargo test filtered_tool_manager_rejects_unknown_permission_mode -- --nocapture
cargo test agent_permission_mode_maps_runtime_permissions -- --nocapture
cargo test agent_permission_mode_accepts_case_insensitive_aliases -- --nocapture
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets -- -D warnings
git diff --check
```

## Follow-Ups

- Implement `AgentDefinition.token_budget` enforcement.
