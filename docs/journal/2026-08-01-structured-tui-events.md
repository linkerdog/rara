# Structured TUI Runtime Events

## What changed

Query, compact, and review task callbacks now deliver `RuntimeControlEvent`
values directly to the TUI task channel. The TUI consumes structured
`RuntimeEvent` variants for assistant output, tool lifecycle, memory notices,
todo updates, warnings, and errors.

The old AgentEvent-to-role/message conversion remains only as a test adapter
for legacy renderer tests. Runtime task production no longer uses it to derive
TUI semantics. OAuth and model-download progress continue to use transcript
events because those messages are presentation-only status.

## Why

Formatted transcript messages are a lossy compatibility surface. Parsing their
roles and prefixes in the TUI duplicated runtime semantics and made changes to
tool output formatting capable of changing behavior. RuntimeControlEvent
already carries the typed event family, provenance, and sequence identity, so
the TUI should consume that contract directly.

## Trade-offs

Tool result payloads may still contain user-visible delegated-agent text whose
content must be interpreted for request-input display. This parsing is scoped
to the typed `ToolEvent::Result` payload and is not a role/message dispatch
mechanism.

## Verification

- `cargo fmt --all`
- `cargo check --all-targets`
- `cargo test --bin rara tui::runtime::events --no-fail-fast`
- `git diff --check`

## Remaining work

Remove the legacy test-only AgentEvent adapter after the renderer test suite
is migrated to construct structured runtime events directly.
