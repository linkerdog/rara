# Typed Tool Summary Projection

## What changed

The active-turn TUI tool summary now consumes `ToolTranscriptPayload` entries
before inspecting legacy role strings. Running and completed entries are
paired by provider or runtime-assigned `call_id`, with the tool name as the
fallback identity. Legacy role/message parsing remains available for older
persisted transcript entries.

## Why

OpenCode keeps tool lifecycle data in structured session parts with stable
identity. Pairing tool output through role strings makes repeated or concurrent
calls harder to represent correctly and keeps runtime semantics in the display
layer. This change makes the TUI follow the typed runtime projection while
preserving compatibility with existing transcripts.

## Trade-offs

The summary still contains a legacy parser until all persisted transcript
writers emit typed payloads. Typed entries are authoritative and do not depend
on their role strings, while legacy entries retain the previous behavior.

## Verification

- `cargo fmt --all`
- `cargo test --bin rara tool_summary_uses_typed_tool_identity_before_role_strings --no-fail-fast`
- `cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings`
