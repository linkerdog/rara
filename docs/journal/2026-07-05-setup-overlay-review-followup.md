# Setup Overlay Review Follow-Up

## Summary

The setup overlay split keeps the renamed `overlay::setup` module, but restores
the user-visible picker and provider setup behavior that regressed during the
cleanup.

## Scope

- Skills picker rows now render as compact single-line entries with name, scope,
  enabled state, and title.
- The skills picker uses a stateful list with an explicit visible offset so the
  selected entry stays in view after keyboard navigation.
- API key setup copy is provider-aware again for OpenAI-compatible, DeepSeek,
  and Codex flows.
- Setup editor wrapping now passes the modal width to the shared wrapping helper
  instead of subtracting padding twice.

## Validation

```bash
cargo test --locked tui::render::tests
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked
```

## Follow-Ups

None.
