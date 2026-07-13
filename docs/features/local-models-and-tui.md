# Local Models And TUI Specification

## Problem

RARA started with hosted-provider assumptions and a simple setup-oriented TUI. Local-model support,
download lifecycle, and model switching now exist, but the product contract is still implicit and
partly encoded in runtime code paths instead of stable docs.

## Scope

- Local model provider architecture.
- Supported local model preset families.
- Download and cache behavior.
- Current TUI interaction contract for model selection.
- Planned interaction direction for inline command-driven TUI control.

## Non-Goals

- Full multimodal model support.
- Detailed benchmark policy.
- Published user documentation.

## Architecture

### 1) Backend Integration

- All model backends implement the shared `LlmBackend` trait.
- Local inference is implemented by `LocalLlmBackend`.
- Local backends must remain pluggable through the same agent/tool loop used by hosted providers.
- Local tool-calling currently works through a constrained JSON response shim that is converted into
  the internal tool-use content model.

### 2) Model Families

Current local model presets:

- `gemma4-e4b`
- `gemma4-e2b`
- `qwen3-8b`

Provider aliases may resolve to the same local backend path so the CLI and TUI can stay ergonomic.

### 3) Download And Cache Behavior

- Model artifacts are downloaded through `hf-hub`.
- Download progress should remain visible to the operator.
- The default cache root is user-global and persistent.
- `RARA_MODEL_CACHE` may override the default cache location.
- The runtime uses `hf-hub`'s blocking typed model client so synchronous backend construction remains safe inside
  RARA's async runtime. Downloads keep the established `models--owner--name` cache layout, preserving reusable
  snapshots across the client migration.

### 4) TUI Interaction Direction

Current state:

- the TUI includes a setup screen that can switch local model presets;
- local providers do not require an API key;
- model changes rebuild the backend and agent in-process.

Target state:

- common runtime actions should move into slash commands inside the main prompt flow;
- setup should become a first-run onboarding and fallback configuration surface, not the primary UX;
- the TUI should expose current provider/model/runtime status continuously.

## Contracts

### 1) Local Provider Contract

- `local`, `local-candle`, `gemma4`, `qwen3`, and `qwn3` resolve through the local backend path.
- Local providers must not require an API key to enter the TUI or start a chat session.
- Revision selection should remain configurable because Candle integration tracks upstream `main`.

### 2) Agent Loop Contract

- Local model integration must preserve the existing agent loop entry points.
- Tool-call responses must be translated into the same internal content representation used by other
  backends.
- Backend-specific prompt shaping should stay encapsulated inside the backend implementation.

### 3) TUI Contract

- The operator must be able to identify the current provider and model without leaving the main chat flow.
- The operator must be able to switch local presets without restarting the binary.
- Long-running local actions such as downloads should produce visible progress.

## Validation Matrix

- `cargo check`
- focused local-model-server cache snapshot tests
- focused backend unit tests for alias resolution and tool-call parsing
- focused TUI tests or manual validation for model switching and local-provider no-key flow

## Open Risks

- The JSON tool-call shim is more brittle than model-native function calling.
- Prompt formatting and stop behavior may diverge across Gemma 4 and Qwen3 variants.
- The current hash embedding fallback is operational but weak for semantic retrieval.
- The setup screen still carries too much product responsibility compared with the intended inline command UX.

## Source Journals

- [2026-04-11-local-model-bootstrap](../journal/2026-04-11-local-model-bootstrap.md)
- [2026-07-13-hf-hub-v1-migration](../journal/2026-07-13-hf-hub-v1-migration.md)
