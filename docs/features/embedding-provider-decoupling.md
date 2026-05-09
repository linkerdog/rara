# Embedding Provider Decoupling

## Problem

RARA's embedding layer currently reuses the chat provider's credentials and
endpoint URL. The `LlmBackend::embed` method constructs the embeddings URL by
appending `/v1/embeddings` to the chat provider's `base_url`. Providers that
only offer chat — DeepSeek, Kimi, and potentially others — do not expose an
embeddings endpoint; the call fails at the HTTP layer.

```
deepseek base_url → https://api.deepseek.com/v1
embedding URL     → https://api.deepseek.com/v1/embeddings  ← 404 / host error
```

Failures bubble up as `retrieve_session_context` and `retrieve_experience`
returning empty results, silently degrading memory retrieval.

## Scope

- Decouple the embedding backend from the chat backend so they can be configured
  independently.
- Define a minimal `EmbeddingBackend` contract that the existing `LlmBackend`
  embed methods are refactored into.
- Providers that don't support embeddings (DeepSeek, Kimi) degrade gracefully
  with neither HTTP calls nor panics.
- Retain existing behavior for OpenAI-compatible providers that do support
  embeddings (OpenAI, Ollama, Gemini, Codex).
- **Memory retrieval (`retrieve_session_context`, `retrieve_experience`,
  `remember_experience`, `retrieve_memory`) must continue working when any
  valid embedding provider is configured**, even if the chat provider is
  DeepSeek or Kimi.

## Non-Goals

- Adding a local inference-based embedding backend (Candle, ONNX) in this spec.
  The Near-Term Focus item "A real embedding backend for local memory
  retrieval quality" is a separate project.
- Changing the LanceDB memory storage format.
- Adding embedding provider selection to the TUI setup flow (spec-only; TUI
  integration is follow-up).

## Architecture

### 1) EmbeddingBackend Trait

Extract a standalone trait, separate from `LlmBackend`:

```rust
#[async_trait]
pub trait EmbeddingBackend: Send + Sync {
    /// Compute a vector embedding for `text`.
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;
}
```

- `OpenAiEmbeddingBackend` — wraps the existing `openai_compatible::embed` logic.
  Configured with its own `base_url`, `api_key`, and `model`.
- `GeminiEmbeddingBackend` — wraps the existing `gemini::embed`.
- `OllamaEmbeddingBackend` — wraps the existing `ollama::embed`.
- `NoopEmbeddingBackend` — returns `Err("embedding not configured")` for
  providers that don't support embeddings.

Memory consumers (`memory_store.rs`, `context/retriever.rs`, `session.rs`,
`context/assembler.rs`) call `EmbeddingBackend::embed` instead of
`LlmBackend::embed`.

### 2) Provider Detection

`OpenAiEndpointKind` already disambiguates Deepseek vs standard OpenAI-compatible
providers. During backend construction:

| Chat Provider | Embedding Behavior |
|---|---|
| OpenAI / OpenAI-compatible | `OpenAiEmbeddingBackend` with same `base_url` |
| DeepSeek / Kimi | `NoopEmbeddingBackend` |
| Codex | Reuses parent `openai_compatible::embed` (already delegates internally) |
| Gemini | `GeminiEmbeddingBackend` |
| Ollama | `OllamaEmbeddingBackend` |
| Bedrock | `NoopEmbeddingBackend` (stubbed today) |
| Local/Candle | `NoopEmbeddingBackend` |

### 3) Independent Config (Future)

A dedicated embedding config block allows manual override when the chat provider
doesn't support embeddings but the user has a separate embedding service:

```toml
[embedding]
provider = "openai"       # or "gemini", "ollama", "none"
model = "text-embedding-3-small"
base_url = "https://api.openai.com/v1"
api_key = "sk-..."
```

When `provider = "none"`, memory retrieval degrades to keyword-only search
(already supported by LanceDB FTS). This config is deferred to a follow-up PR;
the current spec only requires that the default behavior selects a working
embedding backend per provider.

### 4) Fallback Paths

The retriever already has a keyword-only fallback:

```rust
// context/retriever.rs:52
let Ok(query_vector) = self.backend.embed(query).await else {
    // keyword fallback
};
```

This path already works when `embed` returns `Err`. The `NoopEmbeddingBackend`
triggers this naturally.

## Implementation Contract

### Phase 1: Trait Extraction (this PR)

1. Define `EmbeddingBackend` trait in `src/llm/shared.rs` (or new
   `src/llm/embedding.rs`).
2. Move each backend's `embed` method from `LlmBackend` to its own
   `EmbeddingBackend` impl block.
3. In the `BootstrapRuntime`, construct the embedding backend separately from
   the chat backend, selecting `NoopEmbeddingBackend` for DeepSeek/Kimi.
4. Update call sites in `memory_store.rs`, `retriever.rs`, `session.rs`,
   `assembler.rs` to use the new `EmbeddingBackend` reference.
5. Remove `embed` from `LlmBackend`.

### Phase 2: Config (follow-up)

1. Add `[embedding]` section to `RaraConfig`.
2. Add `/embedding` command to TUI.
3. CLI: `--embedding-provider` and `--embedding-model` flags.

## Verification

- `cargo test` — existing tests pass with `NoopEmbeddingBackend`.
- Integration test: DeepSeek chat provider + OpenAI embedding → memory retrieval
  produces non-empty results.
- Integration test: DeepSeek chat provider + no embedding → keyword fallback.
