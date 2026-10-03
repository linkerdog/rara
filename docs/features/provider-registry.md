# Provider Registry And Layered Configuration

## Problem

Provider identity currently shares a representation with transport selection.
Endpoint profiles carry only one model, so adding a compatible service requires
configuration and picker changes even when its wire protocol is already supported.

## Scope

Add a runtime provider registry with OpenCode-style `provider`, `models`,
`options.baseURL`, `options.apiKey`, `model`, and provider/model filters. Retain
legacy `config.json` and its existing provider flows. Provider configuration is
owned by the config/runtime layer; the TUI consumes its resolved catalog.

## Non-Goals

This first implementation does not import OpenCode credentials or execute npm
packages. It does not claim native Anthropic, Azure, Cohere, Copilot OAuth,
embedding, or reranking support. Existing Codex, Bedrock, Gemini, Ollama, and
Candle paths remain available. Remote organization configuration, managed system
policy, and model variants are outside this rollout.
Cross-provider `small_model` is rejected explicitly; same-provider references
select the auxiliary model through the existing summary path. JSON documents
cover provider/model configuration only, not OpenCode's unrelated settings.

## Architecture

Keep `LlmBackend` as the execution boundary. A provider ID names a connection;
the transport names a wire protocol. Compatible providers reuse the existing
chat-completions adapter. Catalog metadata must not imply live verification.

Rig's provider capability separation and shared compatible adapter inform this
boundary. OpenCode's configuration merge, model identity, and explicit provider
options inform the user contract. Codex's provider registry also separates
connection metadata from wire protocol; Claude Code resolves an explicit model
before starting a task.

## Contracts

- Read legacy configuration first, then merge provider documents from global
  `rara.json`/`rara.jsonc` beside `config.json`, `RARA_CONFIG`, project
  `rara.json`/`rara.jsonc` (Git root through working directory), and
  `RARA_CONFIG_CONTENT`, in that order. JSONC wins over JSON at the same level.
  CLI selection overrides the resolved document. Objects merge recursively;
  arrays and scalar values replace earlier values.
- Provider documents accept JSON comments and trailing commas. Expand `{env:NAME}`
  and `{file:path}` in string values, with file paths relative to the declaring
  document. Missing files are errors. Missing environment variables resolve to
  empty strings; an empty key does not establish a connection.
- Model references split on the first slash only: `provider/model/id` selects
  provider `provider`, model `model/id`. A model's optional `id` is its wire ID;
  its map key remains the selection identity. `name` is display-only.
- `options.baseURL` is the complete API root, including any version segment.
  Requests append `/chat/completions`; they never insert an extra `/v1`.
- `enabled_providers` restricts the registry; `disabled_providers` wins.
  A provider's `whitelist` narrows model IDs and `blacklist` removes from that set.
  Hidden or unknown selections fail explicitly rather than falling back.
- Credentials and options are resolved per provider. Environment/file-derived
  secrets remain runtime-only. Saving legacy TUI preferences must not flatten
  provider documents or resolved secrets into `config.json`.
  Precedence is explicit `options.apiKey`, provider credential environment, then
  `provider-auth.json`. `/connect` writes the separate credential store atomically
  under a file lock; Unix files are created with mode `0600`. Saving a key does
  not override an explicit configuration/environment key.
- Configured models appear in `/model` with their provider and display names.
  Selection triggers the existing backend rebuild. Unauthenticated remote
  providers do not become available merely because another compatible provider
  has a key. Explicit custom endpoints may omit authentication for local servers.
- `limit.context` and `limit.output` control context and output budgets.
  Only explicitly supported request options are accepted; unsupported transports
  fail with an actionable error instead of silently using Chat Completions.
- **Context budgeting**: `OpenAiCompatibleBackend::context_budget`
  (`src/llm/openai_compatible.rs`) reserves `limit.output` in
  `reserved_output_tokens`/`compact_threshold_tokens` whenever
  `max_output_tokens` is set on the backend, regardless of which
  constructor set it — `with_provider_model` (a registry model's
  `limit.output`) or the public `with_max_output_tokens` (opt-in
  measurement tooling: `deepseek_cache_probe.rs`,
  `agent/tests/cache_trial/driver.rs`). This matches
  `chat_completion_request_body`'s own unconditional `max_tokens` send —
  before this fix, the override required `configured_api_root` too (true
  only via `with_provider_model`), so a caller using
  `with_max_output_tokens` alone had that value sent on the wire while
  `context_budget` still reported the generic window-percentage heuristic.
  A `limit.output` grossly larger than the model's actual window is not
  validated here (unlike the DeepSeek Anthropic route's own
  `ensure_output_budget_fits_window` — this shared, far more widely used
  backend has more request call sites, including a separate
  auxiliary/summary model path, and deserves its own separate
  consideration rather than a reflexive copy of that guard). See the dated
  journal entry.
- Explicit `model` wins over the saved recent selection; otherwise an available
  recent registry model is restored. With no legacy selection, the first
  available configured model is used. An explicit model without credentials
  fails before execution; it never falls through to the mock backend. Configure
  credentials before setting an explicit default model.

## Example

Place this document in the project root as `rara.jsonc`:

```jsonc
{
  "model": "groq/fast",
  "provider": {
    "groq": {
      "options": { "apiKey": "{env:GROQ_API_KEY}" },
      "models": {
        "fast": {
          "id": "llama-3.3-70b-versatile",
          "name": "Fast coding model",
          "limit": { "context": 32768, "output": 4096 }
        }
      }
    },
    "local-server": {
      "npm": "@ai-sdk/openai-compatible",
      "name": "Local server",
      "options": { "baseURL": "http://localhost:8000/v1" },
      "models": { "my-model": {} }
    }
  }
}
```

Use `/connect` to store a provider key, `/model` to choose a configured model,
or `--model groq/fast` for an explicit CLI selection. Model map keys may be
aliases; `id` carries the actual upstream model ID. Provider presets supply
API roots and credential environment variables for `openai`, `groq`, `together`,
`xai`, `mistral`, `minimax`, `zai`, `zai-coding-plan`, `hyperbolic`, `moonshotai`,
`deepseek`, and `openrouter`. Users declare model IDs instead of relying on a
compiled list that becomes stale.

Supported model options are `temperature`, `topP`, and `reasoningEffort`.
`npm` may be omitted or equal `@ai-sdk/openai-compatible`; other SDK package
names are rejected rather than dynamically loaded.

## Validation Matrix

| Contract | Evidence |
| --- | --- |
| Layer precedence and deep merge | Temporary global/project/explicit documents |
| JSONC and variable substitution | Comments, escaped quotes, trailing commas, relative files |
| Provider isolation | Two providers with different keys and endpoints |
| Stable model identity | Alias with slash-containing wire ID |
| Filters | Disabled wins; whitelist plus blacklist; unavailable selection error |
| No secret writeback | Load, select, save, inspect persisted files |
| Exact endpoint and wire options | Local HTTP request capture through runtime backend |
| Model picker | Configured names, availability, and selected provider/model |

## Operational Notes

Provider presets describe endpoints and credential variable names, not a promise
that every remotely offered model supports coding-agent tools. Explicit model
configuration permits new model IDs without a binary release. Live credentialed
verification is separate from local protocol tests.

## Open Risks

Native provider protocols and provider-specific tool/reasoning dialects require
separate adapters and credentialed conformance checks. This rollout proves the
shared Chat Completions request path, not every model's remote capabilities.
Global configuration remains under `RARA_HOME` (default `~/.rara`), rather than
moving legacy data to an XDG directory. Authentication removal and live model
discovery for registry providers remain follow-up work.

## Source Journals

- [Provider comparison and rollout](../journal/2026-09-18-provider-registry.md)
- [Context budget must reserve max_output_tokens whenever it's set](../journal/2026-09-24-openai-compatible-context-budget-max-output-tokens-gate.md)
