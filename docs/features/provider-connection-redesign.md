# Provider Connection & Model Selection Redesign

## Summary

Redesign provider connection and model selection to match OpenCode's UX:

1. **Provider connection state** — green dot when credentials exist
2. **`/connect`** — provider picker → auth → verify → list models → select
3. **`/model`** — provider picker → model list of selected provider → select

## Design

### Connection State

```
Sidebar display:
  ● DeepSeek    (connected, green dot)
  ○ OpenAI      (not connected, no dot)
  ● Codex       (connected, green dot)

Green dot = credentials exist for this provider.
No dot = no credentials configured.
```

Implementation: check `AppState` / `RuntimeConfig` for each provider family:
- Codex: OAuth token present
- DeepSeek: API key in config
- OpenAI: at least one profile with API key
- Ollama: endpoint reachable (always connected if configured)
- Candle: always present (local)

### /connect Flow

```
/provider picker/ → select provider:

  has credentials?
  ├─ yes → try list models
  │        ├─ success → show models, optionally select one
  │        └─ fail → show auth UI
  └─ no → show auth UI
           ├─ API key input  (DeepSeek, OpenAI, custom)
           └─ OAuth flow     (Codex)
           success → list models → optionally select
```

### /model Flow

```
/provider picker (with green dots)/ → select provider →

/model list/:
  deepseek-chat       64K
  deepseek-reasoner   64K
  deepseek-v4-flash    1M
  deepseek-v4-pro      1M

select → set as current model
```

### UI Style

Match Command Palette style (bordered block, badge title, highlight symbol).

Provider picker: each row shows `●` or `○` + provider name.
Model list: each row shows model name + context window size.

## Implementation Plan

### Phase 1 — Connection state (P0)
- Add `fn is_provider_connected(app: &TuiApp, family: ProviderFamily) -> bool`
- Add green/red dot rendering to list picker items
- Display connected providers in sidebar

### Phase 2 — Refactor /connect (P0)
- Remove forced config wizard
- Provider picker → check credentials → auth if needed → list models

### Phase 3 — Refactor /model (P1)
- Two-step: provider picker → model list
- Model list shows context window sizes from `MODEL_WINDOWS` map

## Future
- Custom provider support (arbitrary API endpoint + API key)
- Model list caching with manual `/refresh`
