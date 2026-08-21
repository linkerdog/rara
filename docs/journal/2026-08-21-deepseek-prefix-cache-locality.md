# DeepSeek Prefix Cache Locality

## Summary

RARA now keeps its model-visible prompt history append-only for ordinary turns.
Environment, execution mode, protocol/LSP sources, and selected retrieved memory
move out of the system message into persisted typed context on the current user
message. DeepSeek continues to use its official OpenAI-compatible endpoint and
automatic prefix-cache accounting.

## Background

The previous `__DYNAMIC_BOUNDARY__` marker was internal text, not a provider
cache boundary. OpenAI-compatible serialization flattened the system content,
dropped the attached Anthropic-style cache hint, and still sent volatile
environment and mode text inside the first message.

There was a second prefix-locality failure: retrieved memory and projected tool
results were applied only to a request copy. A later request reconstructed old
history without the exact content seen by the previous model call, so the
provider could not reuse the earlier prefix beyond the system message.

The design was compared with two upstream patterns before implementation:

- Claude Code can move per-machine dynamic system sections into the first user
  message to improve shared prompt-cache locality.
- Codex persists typed world-state context and appends changes instead of
  rebuilding earlier message content on every turn.

## Scope

- Removed the synthetic dynamic-boundary marker and generic `cache_control`.
- Kept project instructions, stable workspace memory, skills, language
  guidance, append text, and child capability policy in the system prompt.
- Added `rara_model_context` blocks for environment, execution mode,
  protocol/LSP sources, and retrieved memory.
- Persisted those blocks with the current user message while excluding them
  from human transcript and memory-query text.
- Added provider rendering for OpenAI-compatible chat, Codex Responses,
  Ollama, Gemini, Bedrock, and local prompts.
- Disabled request-only tool-result projection when a backend declares
  automatic prefix caching without cache edit. Durable compaction remains the
  bounded-history mechanism.

## Key Decisions

1. Prefix stability is a request-history invariant, not a marker in prompt
   text.
2. Query-dependent memory remains in the current-user suffix, but the exact
   model-visible attachment is retained in later history.
3. Mode and protocol changes append deltas. Clearing protocol sources appends
   an explicit cleared state so old instructions do not remain current.
4. Stable project source changes may intentionally produce a new cold system
   prefix; RARA does not freeze changed repository instructions for cache hits.
5. DeepSeek uses the official `chat/completions` interface. RARA relies on the
   provider's automatic cache and parses `prompt_cache_hit_tokens` and
   `prompt_cache_miss_tokens`; it does not emulate Anthropic cache controls.

## Validation

Executed locally:

```bash
cargo fmt --all
cargo test -p rara-instructions
cargo test -p rara model_context::tests::model_context_is_upserted_before_user_text -- --nocapture
cargo test -p rara agent::tests::prompt_cache::later_request_preserves_the_previous_model_visible_prefix -- --nocapture
cargo test -p rara agent::tests::microcompact::automatic_prefix_cache_preserves_tool_results_until_durable_compaction -- --nocapture
cargo test -p rara agent::tests::context_view::query_persists_selected_memory_as_typed_model_context -- --nocapture
cargo test -p rara agent::tests::context_view::protocol_prompt_registry_feeds_prompt_runtime_for_query -- --nocapture
cargo test -p rara llm::model_context_tests -- --nocapture
cargo test --workspace --quiet
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
```

The focused tests, the complete workspace test suite, workspace check, and
warning-denying Clippy run passed. The root library suite completed 1,325 tests.
On macOS, the root test binary emitted the existing linker notice about the
compact-unwind `__eh_frame` size; all tests still completed successfully.

## Follow-Ups

- Run an opt-in A/B test against the official DeepSeek API and compare cache
  hit/miss usage across repeated turns. No live provider call was made in this
  implementation checkpoint.
- Evaluate whether mode-specific tool-set changes should use a stable
  capability envelope. Runtime enforcement must remain authoritative before
  changing that request shape.
