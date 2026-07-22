# Plugin Command Registry

## Summary

Plugin `commands/**/*.md` files now load into the session plugin runtime as
compact command summaries. The summaries are owned by runtime bootstrap and are
attached to the agent alongside plugin hooks and plugin skill summaries, so
presentation surfaces can report loaded command metadata without scanning
plugin directories themselves.

## Key Decisions

- Plugin command names are namespaced as `plugin_name:command_name`.
- Nested command files preserve their relative directory path with `/`
  separators, for example `plugin:git/review`.
- Command descriptions come from leading frontmatter `description` when
  present, otherwise from the first non-heading markdown body line.
- Plugin command summaries are not routed through the TUI local slash-command
  parser yet. Invocation remains deferred until RARA has a shared command
  execution contract outside the TUI presentation layer.
- `/status` displays the loaded plugin command count from the runtime snapshot.

## Validation

- `cargo test plugin_middleware::tests::registers_project_plugin_command_summaries -- --nocapture`
- `cargo test tui::status_display::tests::overview_status_reports_agent_extension_details -- --nocapture`

## Follow-Ups

- Route plugin skill invocation/reload through a shared runtime registry.
- Register plugin agent definitions through the runtime-owned agent registry.
- Continue control-plane readiness work for plugin-provided extension sources.
