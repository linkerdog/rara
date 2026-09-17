# Provider Registry And OpenCode Configuration Comparison

## Summary

Introduce a configuration-owned registry for compatible providers and explicit
model maps. Keep `LlmBackend`, the existing provider implementations, and legacy
configuration as compatibility boundaries. The canonical contract is
[provider-registry.md](../features/provider-registry.md); interaction details
are in [provider-models.md](../interaction/provider-models.md).

## Reference Review

Sources inspected before implementation:

- [Rig provider reference](https://docs.rs/rig-core/0.42.0/rig_core/providers/index.html),
  plus local source at `b42ce15d2c590b0af817fc6f3dafc2177e7c8268`:
  `providers/mod.rs`, compatible adapter guidance, and individual provider roots
  and environment key declarations.
- [OpenCode config](https://opencode.ai/docs/config/),
  [providers](https://opencode.ai/docs/providers/), and
  [models](https://opencode.ai/docs/models/), plus source at
  `4643e65ad6334de3e4e68dedc201d5fbb828c9fe`: config layer ordering/deep merge;
  provider options, model aliases, filters, and first-slash model references.
- Codex source at `ea2046f36d5ee12d39c8e168fc3e5129301afa2b`:
  `codex-rs/model-provider-info/src/lib.rs` separates provider definitions from
  wire protocol and environment credentials.
- Claude Code source at `4b9d30f7953273e567a18eb819f4eddd45fcc877`:
  `src/main.tsx` resolves explicit CLI/environment models before task execution.

## Coverage Comparison

| Rig integration group | Existing runtime | This rollout |
| --- | --- | --- |
| OpenAI Chat, DeepSeek, Moonshot, OpenRouter | Compatible backend and selected profiles | Declarative multi-model providers; scoped keys/options |
| Groq, Together, xAI, Mistral, MiniMax, Z.ai, Hyperbolic | Manual generic endpoint possible | Named endpoint/key presets through the shared compatible backend |
| Gemini, Ollama, Bedrock, local inference | Existing backend paths | Preserved |
| Anthropic Messages, Azure, Cohere | No equivalent generic native adapter | Explicit follow-up, not mapped silently to Chat Completions |
| Copilot/ChatGPT subscription auth | Separate Codex auth only | Existing Codex path preserved; no new subscription auth |
| Hugging Face, Venice, Llamafile, other compatible endpoints | Manual endpoints | Custom provider plus explicit model map |
| Voyage/embedding/reranking | Separate memory/embedding subsystem | Outside coding-model selection |

The preset count is not a remote model conformance claim. Named services still
need credentialed tool/stream/reasoning verification for their individual models.

## Plan And Implementation

1. Define the config and interaction contracts, preserving legacy readers.
2. Resolve JSONC layers, string variables, provider identities, model aliases,
   credential precedence, and filters in the config crate.
3. Route configured models through the existing adapter with an exact API root.
   Preserve the active model when storing another provider's credential.
4. Add scoped configuration, runtime HTTP, and TUI regression coverage.

Key implementation findings:

- Automatically appending `/v1` breaks versioned roots such as Z.ai's `/v4`.
  Configured API roots now append only the operation path.
- TUI startup previously reloaded legacy configuration instead of consuming the
  runtime's resolved configuration. Startup now passes that same configuration
  into TUI initialization.
- Saving a projected runtime configuration must not persist expanded secrets or
  overwrite provider documents. Recent selection and explicit credentials have
  separate state files; legacy provider settings remain editable.
- Credential updates use a lock and atomic rename so concurrent sessions do not
  lose unrelated provider keys. Explicit options/environment keys retain priority.

## Validation

Focused commands:

```bash
cargo test -p rara-config --lib
cargo test --locked --lib configured_provider
cargo test --locked --lib tui::
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```

The runtime regression captures a real local HTTP request produced by the
runtime backend factory, including exact path, wire model ID, authorization,
context/output limits, and sampling/reasoning options. TUI tests cover immutable
credential targets, names, availability isolation, masking, and selection.
Configuration tests: 63 passed. The TUI suite passed 652 tests; the final
provider-focused library run passed 70 tests, including the added credential
isolation and provider-family regression checks. Strict workspace Clippy,
formatting, and diff checks passed. The macOS test linker reports the existing
`__eh_frame` size warning; no new Rust or Clippy warnings were introduced.

## Follow-Ups

See [the provider backlog](../todo.md#provider-coverage). Native protocol adapters,
live discovery, credential removal, cross-provider auxiliary models, and variants
remain separate work. No credentialed remote inference or real terminal smoke
is claimed by the local fixtures.
