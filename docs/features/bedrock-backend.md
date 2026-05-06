# Bedrock Backend Boundary

## Problem

RARA supports Amazon Bedrock through the AWS SDK Converse API. The original
implementation kept SDK client construction, Bedrock request conversion, context
budget defaults, and the `LlmBackend` adapter in `src/llm/bedrock.rs`.

That made the provider transport harder to evolve independently from the agent
runtime and kept AWS SDK dependencies attached directly to the main crate.

## Scope

- Keep the current Bedrock SDK Converse path.
- Move Bedrock transport and Bedrock-native conversion into `rara-bedrock`.
- Keep the main crate responsible for adapting RARA runtime types to the
  provider crate.
- Preserve the existing provider name, model configuration, and AWS SDK
  credential-chain behavior.

## Non-Goals

- Do not add Codex-style Bedrock Mantle or OpenAI-compatible SigV4 transport.
- Do not add AWS profile configuration in this change.
- Do not change provider selection, TUI model presets, or user-facing Bedrock
  configuration.
- Do not add Bedrock embeddings.

## Architecture

`rara-bedrock` owns the AWS SDK boundary:

- `BedrockConverseClient`
- `BedrockChatMessage`
- `BedrockChatContent`
- `BedrockToolSpec`
- `BedrockChatResponse`
- context-window defaults for known Bedrock model families

The main crate keeps a thin adapter in `src/llm/bedrock.rs`:

- converts `agent::Message` into `BedrockChatMessage`;
- converts tool schemas into `BedrockToolSpec`;
- converts provider responses into `ContentBlock`, `LlmResponse`, and
  `TokenUsage`;
- implements `LlmBackend` for RARA's runtime.

This split avoids a circular dependency. `LlmBackend` and `Message` still live
in the main crate, so the provider crate cannot implement the runtime trait
directly.

## Contracts

- `provider = "bedrock"` still builds `BedrockBackend` through
  `runtime_context::build_backend_with_progress`.
- Bedrock requests still use `aws_sdk_bedrockruntime::Client::converse`.
- AWS credentials and signing remain delegated to the AWS SDK default chain.
- Unsupported RARA message roles are skipped before provider conversion, as
  before.
- Provider usage maps to RARA usage with zero cache-hit and cache-miss tokens
  because Bedrock Converse does not currently expose those fields through this
  adapter.

## Validation Matrix

- `cargo test -p rara-bedrock -- --nocapture`
- `cargo test llm::bedrock -- --nocapture`
- `cargo fmt --check`
- `cargo check`

## Open Risks

- The adapter boundary still depends on RARA runtime types in the main crate.
  A later shared runtime-types crate would allow provider crates to implement
  more of the backend contract directly.
- AWS profile support remains a follow-up configuration task.
- Streaming support remains inherited from the default `LlmBackend`
  non-streaming fallback for this provider.

## Source Journals

- [2026-05-06-bedrock-backend-crate](../journal/2026-05-06-bedrock-backend-crate.md)
