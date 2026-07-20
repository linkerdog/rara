# Plugin Hook Matchers

## Summary

RARA now evaluates Claude plugin tool hook matchers before executing command
hooks for `PreToolUse` and `PostToolUse` events.

## Background

The plugin loader already parsed `hooks/hooks.json`, but matcher groups were not
preserved on the registered hook handlers and plugin middleware executed every
tool hook for every tool event. This was enough for early registration tests but
too broad for real Claude plugin compatibility.

## Scope

- Preserved group-level `matcher` values on hook handlers.
- Kept handler-level `matcher` values as the more specific override.
- Evaluated tool-name matchers before spawning plugin command hooks.
- Supported empty matcher and `*` as match-all, case-insensitive exact tool
  names, Claude-style `ToolName(...)` patterns by tool name, and `|` or `,`
  alternatives.
- Kept input-level glob evaluation, blocking hook results, `SessionEnd`, and
  output observability out of this slice.

## Validation

```bash
cargo test -p rara-plugins registered_hooks_inherit_group_matcher_unless_handler_overrides -- --nocapture
cargo test plugin_middleware::tests::hook_matcher_filters_tool_events_by_tool_name -- --nocapture
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings
cargo fmt --check
git diff --check
```

## Follow-Ups

- Add input-level matcher evaluation if RARA needs Claude's full tool-input
  glob semantics.
- Implement `SessionEnd`, blocking hook results, and hook output observability.
