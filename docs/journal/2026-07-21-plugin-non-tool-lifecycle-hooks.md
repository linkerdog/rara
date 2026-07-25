# Plugin Non-Tool Lifecycle Hooks

## Summary

RARA now dispatches plugin command hooks for `SessionStart` and
`UserPromptSubmit` from the agent query path. This extends plugin lifecycle
coverage beyond tool hooks and `SessionEnd` while keeping execution ownership in
the app-server runtime rather than the TUI.

## Scope

- Added direct plugin hook runtime entry points for `SessionStart` and
  `UserPromptSubmit`.
- Fired `SessionStart` once per `Agent` instance before the first query mutates
  conversation history.
- Fired `UserPromptSubmit` once per submitted query with the prompt included in
  hook stdin JSON.
- Extended command-hook input with an optional `prompt` field.
- Added an agent-level regression that proves `SessionStart` runs once and
  `UserPromptSubmit` runs once per query.

## Key Decisions

- Non-tool lifecycle hooks are invoked directly by the agent instead of routed
  through the async event-bus callback. The callback path only receives
  concrete runtime events that can be translated from tool or stop activity.
- `SessionStart` is an agent-session lifecycle event, not a process lifecycle
  event. A resumed or rebuilt runtime receives a new agent instance and may fire
  it again.
- `UserPromptSubmit` receives the exact submitted prompt before compaction and
  history repair run for that turn.
- These hooks are currently fire-and-log only. A hook failure does not block the
  user turn, and hook stdout is not injected into model context until a
  structured hook-output surface is introduced.

## Validation

```bash
cargo test agent::tests::plugin_hooks::plugin_non_tool_lifecycle_hooks_run_from_agent_query -- --nocapture
cargo test plugin_middleware::tests -- --nocapture
cargo test -p rara-plugins
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings
cargo fmt --check
git diff --check
```

## Follow-Ups

- Plugin extension registries were completed in
  `docs/journal/2026-07-25-extension-completion.md`.

## Structured Output Observability

RARA now publishes non-blocking lifecycle hook command output as structured
control-plane events. `SessionStart`, `UserPromptSubmit`, and `SessionEnd`
command hooks emit `RuntimeEvent::Hook(command_output)` when they produce
stdout, stderr, or a failed execution result.

The event includes plugin name, hook event name, stdout, stderr, exit code,
timeout state, and success state. This keeps lifecycle hook output observable to
app-server/control-plane subscribers without injecting that output into model
context or changing hook blocking semantics.

## Additional Validation

```bash
cargo test plugin_middleware::tests::lifecycle_hook_output_is_published_as_structured_control_event -- --nocapture
cargo test runtime_control::tests::hook_command_output_uses_structured_wire_shape -- --nocapture
cargo check --locked --workspace --all-targets
```
