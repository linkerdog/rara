# 2026-07-05 · Configuration TODO cleanup

## Summary

This checkpoint closes stale configuration follow-ups that had already become
either implemented contracts or misleading placeholders.

## Scope

- Removed the thread-goal evaluator placeholder feedback from the TUI goal loop.
  Goal conditions remain stored as user intent, but RARA no longer injects a
  fake evaluator result that always says the goal is incomplete.
- Kept `provider=gemini` on the current OpenAI-compatible AI Studio path and
  removed the unused native Gemini API-key backend constructor and auth variant.
  The native Gemini backend is now only the `gemini-code-assist` OAuth protocol
  path.
- Removed the unused TUI Google OAuth task surface. Existing Google OAuth
  credential loading remains available for `gemini-code-assist` runtime startup.
- Marked Codex model catalog picker refresh as an active implemented path rather
  than a reserved future hook.
- Trimmed `docs/todo.md` so the Configuration section only tracks remaining
  active work.

## Validation

Planned validation for this slice:

```bash
cargo fmt
cargo test --locked auth_mode_picker
cargo test --locked status_context_reports_configuration_fields
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets -- -D warnings
```

## Follow-Ups

- Explicit embedding enable/disable and provider override controls remain open.
