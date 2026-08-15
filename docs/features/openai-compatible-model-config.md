# OpenAI-Compatible Model Configuration

## Goal

RARA should expose a generic OpenAI-compatible provider path in the inline TUI model flow so a user can point the agent at any OpenAI-compatible endpoint without editing config files by hand.

This path is separate from Codex-auth login. Codex keeps its own auth flow and provider-specific runtime, while generic OpenAI-compatible endpoints use explicit connection settings.

## User Contract

When the user opens `/model`, the provider picker includes:

- `Codex`
- `DeepSeek`
- `Kimi For Coding`
- `Moonshot AI`
- `OpenAI-compatible`
- `Candle Local`
- `Ollama`

`DeepSeek` is shown as its own provider family because its setup and runtime
contract are provider-specific, but the persisted runtime provider remains
`openai-compatible` with `endpoint_kind = "deepseek"`. This keeps backend
construction shared while keeping the TUI surface clear.

`Kimi For Coding` and `Moonshot AI` are separate provider families even though
both use the shared OpenAI-compatible backend. Their credentials and API roots
belong to different services and must never be used as fallbacks for each
other.

When `OpenAI-compatible` is selected, the model picker must allow editing:

- `base URL`
- `API key`
- `model name`

These values are editable directly from the model picker through dedicated shortcuts instead of forcing manual config edits.

When `DeepSeek` is selected, the model picker must:

- expose the active DeepSeek endpoint profile as a dedicated surface, not as a
  generic custom endpoint preset;
- show the current API key status;
- provide an API key action even when a key already exists;
- open the API key editor before model loading when the key is missing;
- allow the user to refresh the model catalog after changing the key;
- keep the selected DeepSeek model visible after switching away to Codex,
  Ollama, local models, or generic OpenAI-compatible profiles.

## Defaults

Selecting the OpenAI-compatible preset sets:

- `provider = "openai-compatible"`
- `model = "gpt-4o-mini"` if no explicit model has been set for that provider
- `base_url = "https://api.openai.com/v1"` if no explicit base URL has been set for that provider
- `revision = None`

The generic OpenAI-compatible backend is then constructed from:

- `api_key`
- `base_url`
- `model`

Selecting the DeepSeek family sets:

- `provider = "openai-compatible"`
- `endpoint_kind = "deepseek"`
- `base_url = "https://api.deepseek.com/v1"` unless a DeepSeek profile override exists
- `model = "deepseek-chat"` unless a DeepSeek profile override exists
- `revision = None`

DeepSeek model discovery should call `GET /models` against the DeepSeek API
root with the active API key. If the model-list request fails, the picker may
fall back to the built-in DeepSeek model list, but the failure must stay visible
as a notice or system message.

Selecting the Moonshot AI profile sets:

- `provider = "openai-compatible"`
- `endpoint_kind = "kimi"`
- `base_url = "https://api.moonshot.ai/v1"` unless a Moonshot profile override exists
- `model = "kimi-k2.6"` unless a Moonshot profile override exists
- `revision = None`

Selecting the Kimi For Coding profile sets:

- `provider = "openai-compatible"`
- `endpoint_kind = "kimi_coding"`
- `base_url = "https://api.kimi.com/coding/v1"` unless a Kimi For Coding profile override exists
- `model = "kimi-for-coding"` unless a Kimi For Coding profile override exists
- `revision = None`

Both profiles use the shared OpenAI-compatible chat-completions backend. The
Moonshot profile accepts its saved key or `MOONSHOT_API_KEY`; the Kimi For
Coding profile accepts its saved key or `KIMI_API_KEY`. These provider-native
environment variables are runtime-only credential sources and must not be
serialized back into `config.json` unless the user explicitly saves them
through the normal API-key editor. `KIMI_API_KEY` must not authenticate the
Moonshot profile, and `MOONSHOT_API_KEY` must not authenticate the Kimi For
Coding profile.

For thinking-capable DeepSeek models, RARA sends DeepSeek's documented
thinking-mode controls on chat-completions requests:

- when thinking is explicitly enabled, send `thinking.type = enabled`, and use
  `reasoning_effort = max` for planning-mode or tool-enabled agent requests,
  with Codex-style `low` / `medium` / `xhigh` values normalized to
  DeepSeek's documented `high` / `max` values.

When DeepSeek returns `reasoning_content`, RARA stores it as provider metadata
and replays it only for the DeepSeek endpoint. Assistant messages that contain
tool calls must replay the original `reasoning_content` without trimming or
rewriting it, because DeepSeek requires thinking-mode tool-call turns to be
sent back intact on later requests.

For DeepSeek thinking-capable models, `reasoning_content` is a three-state
history contract:

- missing `reasoning_content` means the assistant turn is legacy history that
  predates RARA's DeepSeek reasoning preservation and may need to be folded
  into an internal context note;
- `reasoning_content = ""` means the assistant turn is a known DeepSeek turn
  whose reasoning slot was present but empty and must still be replayed with an
  explicit empty field;
- non-empty `reasoning_content` must be preserved byte-for-byte.

## Remembered Provider State

RARA keeps provider-scoped remembered state in `RaraConfig.provider_states`.

Switching between providers must preserve and restore provider-local settings instead of overwriting them globally. For the OpenAI-compatible path this includes at least:

- `api_key`
- `base_url`
- `model`
- `revision`
- `reasoning_summary`
- `num_ctx`

This allows the user to switch between Codex, Ollama, local models, and generic OpenAI-compatible endpoints without losing per-provider connection details.

Provider/model switching is a backend hot-swap, not a session reset:

- switching provider/model should not reset the active TUI transcript;
- switching provider/model should not mint a new session id;
- current plan state, pending interactions, and compacted history should survive the backend rebuild.

## TUI Behavior

In the model picker:

- `B` opens the base URL editor for OpenAI-compatible and Ollama
- `A` opens the API key editor for OpenAI-compatible
- `N` opens the model-name editor for OpenAI-compatible
- `Enter` applies the selected preset and rebuilds the backend

The base URL editor and API key editor must use generic OpenAI-compatible wording when the active provider is `openai-compatible`, not Codex-specific auth wording.

The model-name editor is a separate overlay so the user can update the remote model identifier without editing config files.

`/status` should explain the effective model/config surface, including at least:

- the current `model`;
- the current `base_url`;
- the current `reasoning_summary`;
- whether each value came from built-in defaults, provider-scoped state, or legacy global config.
- whether the API key came from the runtime environment variable owned by the
  selected endpoint kind: `MOONSHOT_API_KEY` for Moonshot AI or `KIMI_API_KEY`
  for Kimi For Coding.
- cumulative provider usage cache counters, when the response includes them:
  `cache_hit_tokens`, `cache_miss_tokens`, and
  `cache_hit_rate = hit / (hit + miss)`.

For `codex`, `/status` should also surface the current auth/endpoint shape for the running
provider surface:

- `codex_auth_mode = chatgpt | api_key | -`
- `codex_endpoint_kind = chatgpt_codex | openai_api | unknown | -`

This auth surface is session-stable UI state, not a trigger to reset the current session.

## DeepSeek Chat-Completions Contract

DeepSeek reasoning models may return provider-specific assistant metadata such
as `reasoning_content`. When RARA sends a later chat-completions request that
includes that assistant turn in history, it must preserve and pass back the
provider-required reasoning metadata instead of dropping it during the
`LlmResponse` -> `Message` -> OpenAI-compatible message conversion.

The DeepSeek endpoint kind must therefore support provider-specific assistant
fields in addition to standard `content` and `tool_calls`:

- `reasoning_content` must be retained for the next request when DeepSeek
  returns it;
- `reasoning_content = ""` must remain distinguishable from missing legacy
  history and must be replayed as an explicit empty field on later DeepSeek
  requests;
- reasoning metadata must not be rendered as ordinary assistant prose in the
  committed transcript;
- tool calls must still round-trip with their `id`, `name`, and `arguments`;
- if a provider requires additional assistant metadata for a follow-up request,
  RARA should fail explicitly instead of silently converting the assistant turn
  into an incomplete generic OpenAI-compatible message.

This contract is provider-specific. Generic OpenAI-compatible endpoints should
continue to send only standard fields unless their endpoint kind declares
additional required fields.

## Non-Goals

- This feature does not replace the Codex OAuth flow.
- This feature does not introduce broad provider-specific capability discovery
  beyond explicitly modeled endpoint kinds such as DeepSeek.
- This feature does not validate arbitrary endpoint compatibility beyond normal backend construction errors.
