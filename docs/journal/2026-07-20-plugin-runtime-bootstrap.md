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
- Plugin `PreToolUse` command hooks now also have a session-scoped synchronous
  execution path at the agent tool boundary. `continue:false`, non-zero exits,
  and timeouts return an error tool result to the model and prevent the tool
  from running.
- Plugin `SessionEnd` command hooks run once when the agent loop reaches final
  completion or a hard stop such as max-turn or token-budget exhaustion. The
  agent invokes them directly instead of registering them through the async
  event-bus callback because there is no ordinary tool event to translate.
- `SessionEnd` payloads include empty tool fields, the best available
  `last_assistant_message`, and `is_interrupt: false`. Approval waits remain
  resumable pauses and do not fire `SessionEnd`.
- Cancelled model turns fire `SessionEnd` with `is_interrupt: true` before the
  cancellation error returns to the caller. Other runtime errors keep the
  existing error and recovery behavior so recoverable continuation paths do not
  run cleanup early.
- Plugin `skills/<name>/SKILL.md` directories were initially discovered during
  runtime plugin registration and appended to the agent's available-skill
  summaries. They now load into the shared skill registry; see
  `docs/journal/2026-07-25-extension-completion.md`.
- This slice does not change non-tool lifecycle dispatch beyond `SessionEnd`,
  full structured hook output observability, or plugin extension registry
  invocation/reload ingestion.

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
cargo test agent::tests::plugin_hooks::plugin_pre_tool_use_continue_false_blocks_tool_execution -- --nocapture
cargo test agent::tests::plugin_hooks::plugin_session_end_runs_once_with_last_assistant_message -- --nocapture
cargo test agent::tests::plugin_hooks::plugin_session_end_marks_cancelled_model_turn_as_interrupt -- --nocapture
cargo test plugin_middleware::tests -- --nocapture
```

## Follow-Ups

- Plugin lifecycle, extension registries, and structured readiness follow-ups
  were completed in later plugin journal entries.
