# DeepSeek Empty Reasoning History

## Summary

RARA now preserves DeepSeek `reasoning_content` as a three-state history slot
for assistant turns:

- missing means legacy history that predates DeepSeek reasoning preservation;
- empty string means a known DeepSeek assistant turn with an explicit empty
  reasoning slot;
- non-empty string continues to round-trip byte-for-byte.

## Background

DeepSeek V4 and other thinking-capable DeepSeek models can require assistant
history to keep a `reasoning_content` field even when the field is empty.
RARA previously filtered out empty `reasoning_content`, which collapsed
`missing` and `present-but-empty` into the same state and caused later request
construction to treat fresh assistant turns as legacy history.

## Scope

- Preserve empty `deepseek.reasoning_content` metadata in the OpenAI-compatible
  response parser.
- Replay explicit empty `reasoning_content` back to DeepSeek assistant history.
- Keep true legacy DeepSeek assistant history on the existing fold-to-context
  compatibility path.
- Cover tool-call-only assistant turns so standard OpenAI `tool_calls` and
  DeepSeek DSML tool calls also synthesize an explicit empty reasoning slot.
- Add focused regression coverage for non-streaming, streaming, and DeepSeek V4
  history replay.

## Key Decisions

- The request-side DeepSeek fold path now keys off field presence instead of
  non-empty content. Only truly missing `reasoning_content` counts as legacy.
- Response parsing synthesizes empty DeepSeek reasoning metadata for assistant
  turns that have visible text or tool calls but no explicit reasoning text,
  including tool-call-only turns before those tool calls are appended into the
  final content vector.
- Reasoning-only assistant turns still stay off the replay path because
  request-side empty-assistant filtering continues to ignore metadata-only
  messages.

## Validation

- `cargo test llm::tests::deepseek_visible_text_without_reasoning_content_synthesizes_empty_metadata -- --nocapture`
- `cargo test llm::tests::deepseek_tool_call_only_turn_synthesizes_empty_reasoning_content -- --nocapture`
- `cargo test llm::tests::deepseek_dsml_tool_call_only_turn_synthesizes_empty_reasoning_content -- --nocapture`
- `cargo test llm::tests::deepseek_streaming_visible_text_without_reasoning_content_synthesizes_empty_metadata -- --nocapture`
- `cargo test llm::tests::deepseek_streaming_tool_call_only_turn_synthesizes_empty_reasoning_content -- --nocapture`
- `cargo test llm::tests::deepseek_streaming_dsml_tool_call_only_turn_synthesizes_empty_reasoning_content -- --nocapture`
- `cargo test llm::tests::deepseek_v4_preserves_assistant_history_with_empty_reasoning_content -- --nocapture`
- `cargo test deepseek_ -- --nocapture`
- `cargo check`

## Follow-Ups

- None for the request/response contract itself. Future DeepSeek protocol work
  should extend the same three-state handling if additional assistant metadata
  becomes replay-critical.
