# Extension Completion

## Summary

The remaining extension TODO items are now closed. Plugin skills load into the
shared skill registry, plugin agents load into the runtime agent definition
cache, extension readiness has a structured control-plane event, and embedding
configuration supports explicit provider/local overrides beyond `off` and
`auto`.

## Key Decisions

- Plugin skills use namespaced `plugin_name:skill_name` names and
  `scope: "plugin"`.
- The `skill` tool `reload` action updates the running `SkillManager` instead
  of only validating a fresh manager.
- Plugin agents use namespaced `plugin_name:agent_name` names and enter the
  same session-scoped `AgentDefinitionCache` as native agent definitions.
- Runtime bootstrap publishes `RuntimeEvent::Extension(readiness_updated)` with
  plugin hook, skill, command, agent, and MCP readiness counts.
- `local_embeddings = "provider"` forces the current provider embedding route.
- `local_embeddings = "local"` forces the bundled local sidecar route.

## Validation

- `cargo check --locked --workspace --all-targets`
- `cargo test -p rara-skills plugin_skills_are_namespaced_and_invokable -- --nocapture`
- `cargo test tools::skill::reload_updates_running_manager_with_plugin_skills -- --nocapture`
- `cargo test plugin_middleware::tests::plugin_agent_records_are_namespaced_by_plugin_name -- --nocapture`
- `cargo test runtime_control::tests::extension_readiness_event_uses_structured_wire_shape -- --nocapture`
- `cargo test runtime_context::tests::embedding_route_honors_explicit_provider_and_local_overrides -- --nocapture`

## Follow-Ups

- Plugin command invocation remains deferred until RARA has a shared command
  execution contract outside the TUI local slash-command parser.
