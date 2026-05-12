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

### Phase 1 — Connection state (P0) ✅ DONE (#362)
- [x] Add `fn is_provider_connected(app: &TuiApp, family: ProviderFamily) -> bool`
- [x] Add green/red dot rendering to list picker items
- [x] ~~Display connected providers in sidebar~~ removed — too cluttered

### Phase 2 — Refactor /connect (P0) ✅ DONE (#365)
- [x] Remove forced config wizard
- [x] Connected → notice "Provider is connected ✓"
- [x] Not-connected → auth overlay (ApiKeyEditor, AuthMode, Profile editor)

### Phase 3 — Refactor /model (P1) ✅ DONE (#362)
- [x] ModelSearch overlay with input-driven filtering
- [ ] **Remaining**: show context window sizes from `MODEL_WINDOWS` map in ModelSearch items
- [ ] **Remaining**: model list from API (`/v1/models`) for connected providers, fallback to `MODEL_WINDOWS`

### Phase 4 — Model Catalog And API-List Polish (P1)
- [x] Add Kimi as a first-class provider with catalog-backed model windows.
- [x] Add DeepSeek v4 model-window metadata to the provider catalog.
- [ ] Generalize ModelSearch display so every provider with catalog metadata
      shows context windows consistently.
- [ ] Add provider API model-list loading for connected API-key providers, with
      catalog metadata as fallback and enrichment.

## Future
- Custom provider support (arbitrary API endpoint + API key)
- Model list caching with manual `/refresh`
