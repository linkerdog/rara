# Plugin Runtime Bootstrap Ownership

## Summary

Plugin hook registration now belongs to runtime bootstrap assembly instead of
the TUI rebuild completion path. TUI, ask, print, headless exec, ACP, and Wire
surfaces pass plugin directory options into the same bootstrap path and receive
an already configured runtime.

## Background

The app-server architecture treats TUI as a presentation consumer over the
runtime event bus. Plugin hooks change runtime behavior, so registering them in
TUI task completion made plugin execution a display-layer side effect and left
non-TUI surfaces without the same runtime behavior.

## Scope

- Added `RuntimeBootstrapOptions` with explicit plugin directories.
- Created and started `HookRuntime` during runtime bootstrap.
- Attached the hook runtime to each agent so hook output, tool input mutation,
  and memory-query hooks use the session-owned runtime instead of a process
  global.
- Registered plugin command hooks while converting `RuntimeBootstrap` into an
  agent/runtime handle set.
- Routed TUI rebuild, TUI startup, ask, print, headless exec, ACP, and Wire
  through the same plugin-aware bootstrap options.
- Removed plugin hook registration from TUI rebuild completion.

## Key Decisions

- Plugin discovery source composition remains `user -> project -> explicit`.
- Presentation surfaces may supply runtime options, but they do not own plugin
  scanning or hook registration.
- Agent execution uses a session-scoped hook runtime. The process-global hook
  runtime bridge was removed so runtime behavior cannot silently pin or route
  through the first initialized ACP session.
- This slice does not change `SessionEnd`, blocking hook behavior, hook output
  observability, or plugin extension registry ingestion.

## Validation

```bash
cargo test runtime_context::tests::runtime_bootstrap_options_preserve_plugin_dirs -- --nocapture
cargo test hook_runtime::tests::start_does_not_capture_runtime_event_bus_arc -- --nocapture
cargo test plugin_middleware::tests::plugin_discovery_sources_order_user_project_then_cli -- --nocapture
cargo test plugin_middleware::tests::plugin_callbacks_do_not_retain_hook_runtime_strong_reference -- --nocapture
cargo test app_cli::tests::effective_plugin_dirs_put_cli_dirs_after_config_dirs_and_deduplicates -- --nocapture
cargo test app_cli::tests::clap_parses_explicit_plugin_dirs_as_global_args -- --nocapture
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings
cargo fmt --check
git diff --check
```

## Follow-Ups

- Implement `SessionEnd`, blocking hook results, and hook output observability.
- Feed plugin `.mcp.json`, commands, skills, and agents into structured
  extension registries.
