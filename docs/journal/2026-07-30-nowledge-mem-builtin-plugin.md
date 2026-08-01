# Nowledge Mem Builtin Plugin

## Summary

RARA now ships a builtin `nowledge-mem` plugin that is materialized under the
runtime home directory during plugin discovery. The plugin follows the Nowledge
Mem community Codex plugin shape closely enough to reuse the existing plugin
runtime registries:

- `.codex-plugin/plugin.json` metadata compatibility;
- streamable HTTP MCP registration for the local Nowledge Mem MCP endpoint;
- memory-oriented plugin skills;
- a namespaced `nowledge-mem:nowledge-mem` plugin agent definition.
- config-driven enablement, endpoint URL, and HTTP headers.
- read-only TUI `/status` display for builtin status, endpoint, and local direct
  routing.
- memory query event notices that show the sanitized query preview and result
  count without rendering returned memory content.
- local/cloud mode selection with environment-backed cloud authentication.

## Background

The Nowledge Mem community repository already publishes a Codex plugin with
skills, hooks, and MCP metadata. RARA should not vendor the whole community
repository or make TUI own the plugin. The integration belongs in runtime
assembly so TUI, headless, ACP, Wire, and future app-server surfaces all see the
same plugin state.

## Key Decisions

- Builtin plugin discovery is the lowest-precedence source:
  `builtin -> user -> project -> configured/CLI`. Later sources still override
  earlier plugin names through the existing de-duplication rule.
- Builtin MCP servers use `builtin` provenance and yield to already registered
  user or project MCP servers with the same name. Normal non-builtin plugin MCP
  duplicates remain hard errors.
- Local mode preserves the loopback MCP endpoint. Cloud mode defaults to
  `https://cloud.nowledge.co` and derives `/remote-api/mcp/`. It emits API-key
  and optional space headers as environment-variable references.
- Cloud credentials are persisted in RARA's existing secret configuration field
  but never written to generated plugin files. The runtime exposes the saved
  key through NMEM_API_KEY and keeps the generated transport on an environment
  variable reference.
- `rara mem --api-key <key>` saves the Cloud credential and enables Cloud mode;
  the saved key is restored after restart, and users do not need to manage
  NMEM_API_KEY separately.
- TUI exposes `/mem` as an argument-free picker for disabled, local, or cloud
  mode. It saves the selected mode and requests a runtime rebuild; transport
  construction remains runtime-owned.
- `builtin_plugins.nowledge_mem` controls whether the plugin is materialized
  and which MCP URL or headers are written into its generated `.mcp.json`.
- The builtin plugin accepts Codex-style `.codex-plugin/plugin.json` metadata.
- MCP JSON parsing accepts Codex-style informational `"type": "http"` fields
  and maps the entry to the existing streamable HTTP transport.
- Streamable HTTP MCP transports expose localhost proxy bypass detection for
  `localhost`, `127.0.0.1`, and `::1`. The future HTTP MCP connector must use
  this helper so local Nowledge Mem traffic never goes through system proxies.
- The builtin subagent is a routing agent definition only. It does not expand
  the restricted subagent tool surface with direct shell, MCP, or skill
  invocation.
- TUI status rendering reports the builtin integration from runtime/config
  state only. It does not scan plugin directories or register plugin/MCP
  resources.
- Runtime memory query events carry the original query string so presentation
  surfaces can report what was queried. TUI renders only a sanitized and
  bounded preview, and still suppresses returned memory titles and content.

## Validation

```bash
cargo test -p config loads_codex_style_http_mcp_json_type_field -- --nocapture
cargo test plugin_middleware::tests::builtin_nowledge_mem_plugin_materializes_skills_mcp_and_agent -- --nocapture
cargo test plugin_middleware::tests::appends_builtin_nowledge_mem_mcp_config -- --nocapture
cargo test plugin_middleware::tests::builtin_nowledge_mem_mcp_yields_to_existing_registry_server -- --nocapture
cargo test plugin_middleware::tests::builtin_nowledge_mem_mcp_uses_configured_url_and_headers -- --nocapture
cargo test plugin_middleware::tests::builtin_nowledge_mem_mcp_supports_cloud_auth_without_persisting_secrets -- --nocapture
cargo test plugin_middleware::tests::disabled_builtin_nowledge_mem_plugin_is_not_discovered -- --nocapture
cargo test -p rara-config builtin_nowledge_mem_cloud_mode_derives_remote_mcp_and_env_headers -- --nocapture
cargo test -p rara-plugins loads_codex_plugin_metadata_directory -- --nocapture
cargo test -p rara-config streamable_http_localhost_bypasses_proxy -- --nocapture
cargo test tui::status_display::tests::overview_status_reports_builtin_nowledge_mem -- --nocapture
cargo test tui::status_display::tests::overview_status_reports_disabled_nowledge_mem -- --nocapture
cargo test tui::status_display::tests::overview_status_reports_custom_nowledge_mem_endpoint_and_headers -- --nocapture
cargo test tui::status_display::tests::overview_status_reports_cloud_nowledge_mem_auth_source -- --nocapture
cargo test runtime_control::tests::memory_label_and_metadata_events_use_structured_wire_shape -- --nocapture
cargo test protocol_sources::tests::memory_control_update_delete_and_labels_use_memory_store -- --nocapture
cargo test tui::runtime::events::tests::memory_query_notice -- --nocapture
```

## Follow-Ups

- Decide separately whether subagents should get direct access to plugin skills
  or MCP tools. That changes the child-agent execution authority and should be
  reviewed as a tool-surface change, not hidden inside the builtin plugin.
