# Crate Split

## Current State

The main binary crate carries ~16k lines of top-level `src/*.rs` plus ~20k lines
of `src/tui/`. 9 sub-crates exist but the core logic (agent, session, memory,
control plane, TUI) is still in the binary crate.

Top-heavy files that strain compilation and review:

| File | Lines | Concern |
|------|-------|---------|
| `thread_store.rs` | 1,579 | session persistence |
| `state_db.rs` | 1,406 | durable state |
| `memory_store.rs` | 1,264 | embedding + long-term memory |
| `tool_result.rs` | 1,204 | tool output processing |
| `agent.rs` | 988 | agent loop |
| `runtime_control.rs` | 957 | control plane types |
| `session.rs` | 950 | session management |
| `tui/` (all) | ~20,000 | TUI rendering |

Compilation is monolithic: every change to any file rebuilds all of `rara`, and
every test run depends on the full binary crate.

## Target Structure

```
crates/
  rara-app/          binary crate (thin — just CLI entry, flag parsing, mode dispatch)
  rara-agent/        agent loop, session, transcript
  rara-tui/          Ratatui rendering, widgets, event loop
  rara-memory/       vectordb, memory_store, memory_distiller, embeddings
  rara-tools/        tool registry, execution, result formatting
  rara-control-plane/ runtime_control types, ACP, MCP connection manager
  rara-oauth/        OAuth flows (Google, Codex)
  rara-app-server/   AppServer fan-out bus (future — after agent/render decoupling)
  rara-persistence/  atomic_file, thread_metadata, thread_rollout_log, thread_turn_log
  instructions/      (existing) agent prompt construction
  config/            (existing) configuration model
  provider-catalog/  (existing) LLM provider discovery
  tool-macros/       (existing) proc macros for tools
  bedrock/           (existing) AWS Bedrock backend
  sandbox/           (existing) OS sandbox enforcement
  skills/            (existing) skill file discovery
  file-search/       (existing) workspace file search
  terminal-detection/ (existing) terminal capability detection
```

### Crate Map

```
                           rara-app (binary)
                          /    |    \    \
                    rara-tui  |  rara-control-plane
                              |        |
                         rara-agent   rara-oauth
                         /  |    \
              rara-tools  rara-memory  rara-state
                  |           |
              config    provider-catalog
```

### Dependency Rules

1. **No cycles.** Each crate depends only on crates listed below it in the map.
2. **Leaf crates first** — `rara-state` has zero RARA-internal deps.
3. **Existing crates preserved** — `instructions`, `config`, `provider-catalog`,
   `tool-macros`, `bedrock`, `sandbox`, `skills`, `file-search`,
   `terminal-detection` keep their current roles.
4. **`rara-app` is thin.** It owns CLI argument parsing, selects the runtime mode
   (TUI / ACP / Wire / print), and wires crates together. Business logic lives in
   domain crates.

## Strategy

Split in dependency order: leaf crates first, then interior, then the thin binary.
Each split is an independent PR, each green on `cargo test`. No behavior change.

1. **`rara-persistence`** — `atomic_file.rs`, `thread_metadata.rs`,
   `thread_rollout_log.rs`, `thread_turn_log.rs`, `redaction.rs` (~900 lines).
   True leaf modules, zero internal deps. After this lands, `state_db.rs` and
   `thread_store.rs` can be moved incrementally as their `agent::Message`,
   `memory_store`, `session` dependencies are lifted into separate crates.

2. **`rara-memory`** — `memory_store.rs`, `vectordb.rs`, `memory_distiller.rs`
   (+2,219 lines). Depends on `rara-state`, `config`. Embedding + retrieval.

3. **`rara-tools`** — `tool_result.rs` + tool registry + execution (+1,200+ lines).
   Depends on `config`. Tool lifecycle.

4. **`rara-agent`** — `agent.rs`, `session.rs`, `session_transcript.rs`,
   `session_context.rs` (+3,200 lines). Depends on `rara-memory`, `rara-tools`,
   `rara-state`, `config`. The agent loop itself.

5. **`rara-tui`** — all of `src/tui/` (~20,000 lines). Depends on `rara-agent`,
   `config`. This is the biggest split and the highest-compile-win.

6. **`rara-oauth`** — `oauth.rs`, `google_oauth.rs` (+1,122 lines). Leaf crate.

7. **`rara-control-plane`** — `runtime_control.rs`, `control_plane.rs`, `acp.rs`,
   `mcp_connection_manager.rs`, `mcp_status.rs`, `protocol_sources.rs`,
   `hook_registry.rs` (~2,200 lines). Depends on `rara-agent`, `config`.

8. **`rara-app`** — remove everything except `main.rs`, `app_cli.rs`, `thread_cli.rs`
   (thin ~500 lines). All logic delegated to domain crates.

## What Doesn't Move

- `llm/` — stays in `rara-app` or moves to `rara-agent` depending on coupling.
  Evaluated per split.
- `prompt/` — stays, or absorbed by `instructions` crate.
- `config/` — already a crate, stays.
- `sandbox/` — already a crate, stays.

## Compilation Wins

Every dev cycle (edit → `cargo check` → loop):

| Before | After |
|--------|-------|
| Any change recompiles all 36k lines | Change in `rara-tui` recompiles only `rara-tui` + `rara-app` (thin) |
| Full test suite = single crate | Tests per-crate, parallelizable |
| CI build = monolithic | CI build = 8 crates in parallel, only dirty crates rebuilt |
