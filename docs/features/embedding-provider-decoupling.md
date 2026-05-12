# Embedding Provider Decoupling

## Problem

RARA's vector retrieval path used to depend directly on `LlmBackend::embed`.
That created two concrete failures:

- chat providers without an embeddings surface, such as DeepSeek and Kimi,
  could not support memory retrieval through the main runtime path;
- local and compatibility providers often fell back to weak hashed embeddings,
  so "vector search" existed structurally but not semantically.

The result was inconsistent memory behavior across providers and no clean place
to plug in the local embedding sidecar introduced by
`docs/features/local-embedding-runtimes.md`.

## Scope

- Introduce a standalone `EmbeddingBackend` contract for vector-producing paths.
- Route memory retrieval, memory writes, session-context checkpointing, and
  vector tools through that standalone backend.
- Allow runtime bootstrap to choose between provider-native embeddings and the
  local model-server sidecar independently of the chat backend.
- Define the durable routing policy as sidecar-first unless a provider has an
  explicit native-embedding capability entry.
- Preserve the existing `LlmBackend::embed` implementations as a compatibility
  shim for provider wrappers and tests in this slice.

## Non-Goals

- Removing `LlmBackend::embed` in the same PR. That cleanup is deferred until
  the remaining provider/test surfaces no longer rely on the compatibility
  hook.
- Adding a user-facing embedding-provider picker or config stanza.
- Reworking LanceDB table identity for mixed-profile or mixed-dimension
  embeddings.
- Adding keyword-only recovery to every retrieval path when embeddings are
  unavailable.

## Architecture

### 1) Standalone Embedding Contract

The canonical vector boundary is now:

```rust
#[async_trait]
pub trait EmbeddingBackend: Send + Sync {
    async fn embed(&self, text: &str, kind: EmbeddingInputKind) -> Result<Vec<f32>>;
}
```

`EmbeddingInputKind` distinguishes query vectors from document vectors so the
local sidecar can preserve backend-specific retrieval formatting rules.

### 2) Runtime Routing

Runtime bootstrap constructs the chat backend and embedding backend separately.
The durable routing contract is capability-driven rather than provider-family
heuristics:

| Capability | Embedding route |
|---|---|
| `NativeEmbedding` | Provider-native embedding via `LlmEmbeddingBackend` |
| `NoNativeEmbedding` | `LocalModelServerEmbeddingBackend` |
| `Unknown` | `LocalModelServerEmbeddingBackend` |

Known capability facts:

| Provider surface | Capability | Note |
|---|---|---|
| `deepseek` | `NoNativeEmbedding` | Must not be treated as provider-native embeddings |
| `kimi` | `NoNativeEmbedding` | Must not be treated as provider-native embeddings |

Until the explicit capability registry lands, the current implementation still
uses a provisional provider-name-based route matrix:

| Chat provider | Embedding route |
|---|---|
| `codex` | Provider-native embedding via `LlmEmbeddingBackend` |
| `openai-compatible`, `openrouter`, custom OpenAI-like surfaces | Provider-native embedding via `LlmEmbeddingBackend` |
| `mock` | Provider-native embedding via `LlmEmbeddingBackend` |
| `deepseek`, `kimi` | `LocalModelServerEmbeddingBackend` |
| `gemini`, `gemini-code-assist` | `LocalModelServerEmbeddingBackend` |
| `ollama`, `ollama-native`, `ollama-openai` | `LocalModelServerEmbeddingBackend` |
| `bedrock` | `LocalModelServerEmbeddingBackend` |
| `local`, `local-candle`, `gemma4`, `qwen3`, `qwn3` | `LocalModelServerEmbeddingBackend` |

This means the chat surface and vector surface are no longer forced to share
capabilities even when they still share process-level configuration.

### 3) Local Sidecar Backend

`LocalModelServerEmbeddingBackend` talks to the bundled Python model server's
`POST /v1/embeddings` endpoint and sends:

```json
{"input":"Explain vector search","input_type":"query"}
```

or:

```json
{"input":"Checkpoint text","input_type":"document"}
```

The client caches the last known loopback endpoint, refreshes it through the
existing bootstrap/status path if a request fails, and bypasses system proxies
for loopback requests so local embedding traffic does not get captured by a
global HTTP proxy.

### 4) Wiring Points

The following paths now depend on `EmbeddingBackend`, not directly on
`LlmBackend::embed`:

- `MemoryStore::insert`, `search`, and `update`
- `MemoryRetrievalOrchestrator`
- `retrieve_session_context`
- `remember_experience` and `retrieve_experience`
- per-turn session-context checkpoint writes in `Agent`
- sub-agent and team-created agent runtime construction

`MemoryStore` still retains a chat-backend handle for memory distillation, but
vector indexing/search is a separate dependency.

## Contracts

- Query embeddings must be requested with `EmbeddingInputKind::Query`.
- Durable memory/session text written into vector indexes must use
  `EmbeddingInputKind::Document`.
- Runtime bootstrap must build one embedding backend per process runtime and
  pass it through to the main agent and sub-agents.
- Provider-native embeddings must be enabled only through an explicit capability
  entry or equivalent allowlist, not by assuming that all providers in one chat
  family expose usable embedding models.
- Providers with known missing native embeddings, including `deepseek` and
  `kimi`, must route to the local sidecar.
- Unknown providers must default to the local sidecar until RARA validates and
  records a native embedding capability for that provider surface.
- The local sidecar client must not rely on ambient proxy configuration for
  loopback embedding calls.
- `MemoryStore` may keep a chat-backend reference for distillation, but its
  vector paths must not implicitly call that backend for embeddings when an
  explicit embedding backend is supplied.
- Unsupported or unavailable embedding routes must fail without panic.

## Validation Matrix

| Case | Validation |
|---|---|
| MemoryStore decoupling | `cargo test memory_store::tests -- --nocapture` |
| Local sidecar request shape | `cargo test local_model_server::tests::local_embedding_backend_posts_query_input_type -- --nocapture --test-threads=1` |
| Provider route matrix | `cargo test runtime_context::tests -- --nocapture` |
| Tool/sub-agent wiring compile surface | `cargo test tools::agent_test -- --nocapture` |
| Whole-crate typecheck | `cargo check` |

## Operational Notes

- Provider-native embeddings are still exposed through the legacy
  `LlmBackend::embed` compatibility path in this slice.
- The local sidecar path is now the default recovery route for providers that
  cannot produce useful embeddings themselves.
- The intended steady-state policy is `sidecar-first` with a small explicit
  native-embedding allowlist. Broad provider-family inference is transitional.
- Retrieval paths that depend on a query vector still degrade to "no vector
  candidates" when the configured embedding backend is unavailable; broader
  keyword-only fallback remains follow-up work.

## Open Risks

- The compatibility shim means the chat trait still carries embedding methods,
  so the type boundary is cleaner at runtime than it is at the trait layer.
- The current route matrix is still provider-name based in code. The explicit
  provider capability registry described above remains follow-up work, along
  with user-facing override config.
- Mixed-platform local embeddings still need profile-aware LanceDB identity
  before cross-profile reuse is safe.

## Source Journals

- `docs/journal/2026-05-12-local-embedding-sidecar.md`
