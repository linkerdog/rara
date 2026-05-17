# TODO

Active backlog only. Keep this file small and current.

## Recently Completed (2025-07-15..16)

- [x] uv venv: replace manual Python discovery with `uv venv --python 3.14 --seed` (#423)
- [x] local_model_server file split: 10 include! files, mod.rs facade (#424, #426)
- [x] `/status` embedding fields in System panel (#425)
- [x] SystemMessageKind enum: replace string-prefix matching (#427)
- [x] tui/state/mod.rs split limitation documented (#428)
- [x] Memory spec: session/global files with summary-driven retrieval (#429)
- [x] agent/compact/main.rs split: 1427→451 lines (#430)
- [x] LSP spec: built-in tool design, 4-phase plan (#431)
- [x] Memory Phase 1: file I/O with path-traversal protection (#432)
- [x] Hook registry: remove `#[allow(dead_code)]`, already wired

## Next Up (short-term, ready to implement)

### Memory (Phase 2–4)
- [ ] `summary.md` index: auto-update on session file writes, 5KB retention cap
- [ ] `search_memory`: rg + LanceDB merged search with native fallback
- [ ] Context injection: load summary.md into system context every turn
- [ ] Wire session file creation into runtime startup

### LSP (Phase 1)
- [ ] rust-analyzer bridge: lazy spawn, JSON-RPC handshake, diagnostics parsing
- [ ] `lsp_diagnostics` tool: return cached diagnostics for a file

### File Splits (P0)
- [ ] `tools/bash.rs` (2315 lines) → by tool group
- [ ] `tools/pty.rs` (1649 lines) → stream read/write
- [ ] `tools/agent.rs` (1701 lines) → by tool group
- [ ] `memory_store.rs` (1625 lines) → LanceDB + legacy
- [ ] `thread_store.rs` (1568 lines) → read/write separation
- [ ] `agent.rs` (1339 lines) → tool/plan/history
- [ ] `tui/render.rs` (990 lines) → move to existing submodules
- [ ] `context/assembler.rs` (916 lines) → budget + assembly

### Dead Code Cleanup
- [ ] `runtime_control.rs`: consolidate 40+ individual `#[allow(dead_code)]` to module-level
- [ ] `hook_registry.rs`: remove `#[allow(dead_code)]` from `all_hooks`
- [ ] `acp_consumer.rs`: add scaffolding comment per AGENTS.md
- [ ] `mcp_status.rs`: add scaffolding comment per AGENTS.md
- [ ] `tui/custom_terminal.rs`: add scaffolding comment per AGENTS.md

### Features
- [ ] `rara resume --last`: restore last session after exit
- [ ] Embedding dimension consistency: detect + rebuild on mismatch
- [ ] Sidebar status update: `app.local_model_server` after bootstrap

### Specs to Write
- [ ] Hooks/plugin lifecycle spec
- [ ] Subagent context optimization spec

## Runtime Control Plane / ACP / Wire

- [x] Add a runtime input-control bridge so appserver/ACP/Wire can submit prompts (merged #338+).
- [x] Define adapter-neutral runtime control request/event types.
- [x] Add Claude-style `todo_write` runtime state.
- [x] Add source-aware MCP config registry.
- [x] Route ACP prompt/cancel/session handling through normal runtime path.
- [x] Add MCP connection manager status model.
- [x] Add `/mcp` status surface.
- [x] Publish `/mcp` status snapshots as structured runtime events.
- [x] Add dynamic MCP tool/resource/prompt refresh.
- [x] Add bounded MCP auto-reconnect.
- [x] Add MCP resource references as source objects.
- [x] Add MCP Tool Search.
- [x] Support protocol-registered prompt sources.
- [x] Support protocol-registered skill sources.
- [x] Add protocol-safe memory mutation/query scaffolding.
- [x] Add hook declaration scaffolding.
- [x] Add `support-acp` integration skill.
- [ ] Ensure every new feature is control-plane-ready rather than TUI-only.

## Plugins / Extension Runtime

- [x] Add `rara-plugins` crate for Claude Code plugin discovery.
- [ ] Wire plugin hook registration into runtime startup.
- [ ] Fix plugin lifecycle parity: `SessionEnd` mapping, matcher evaluation, hook stdout/stderr.
- [ ] Add `rara plugin install/list/remove` CLI commands.

## TUI / Composer / Status

- [x] BottomPaneModel struct with structured notice/prompt/input.
- [ ] Finish BottomPaneModel migration: composer/sizing read structured data.
- [ ] Thinking expand/collapse with duration summary.
- [ ] Live bash transcript: streaming output frames.
- [ ] After local embedding sidecar lands: `/status` context fields for backend/model/state.
- [ ] Plugin hook registration feed into TUI runtime status panel.

## Context / Embeddings

- [ ] Record canonical embedding dimension/schema version next to each vector store.
- [ ] Auto-rebuild vector store on dimension mismatch.
- [ ] Make context budgeting model-aware instead of one fixed heuristic.
- [ ] Turn compaction into an explicit runtime lifecycle event.
- [ ] Read project-level AGENTS.md → `project_memory` → inject into context.

## Memory

- [ ] Implement session/global memory files with summary-driven retrieval (Phase 2–4 of spec).
- [ ] Wire `memory_summary` tool.
- [ ] Wire memory into context assembler.
- [ ] `RetrieveMemory` → return real LanceDB results instead of placeholder empty.

## Model Support

- [x] uv venv --python 3.14 --seed for managed Python venv.
- [ ] Add explicit embedding controls: enable/disable, provider override.
- [ ] Claude-style inline `/model` command surface for runtime switching.
- [ ] Add `/status` context fields for model/provider/thread/retrieval/memory/workspace.

## Agent / Tools

- [ ] Refactor ~7 large methods out of `impl TuiApp` to enable file split.
- [ ] Design subagent context budget as a first-class property.
- [ ] Subagent restart/reconnect semantics.
- [ ] Claude plugin runtime integration (long-term).

## Spec Hygiene

- [x] Memory spec: `docs/features/session-global-memory.md`
- [x] LSP spec: `docs/features/lsp-integration.md`
- [ ] Hooks/plugin lifecycle spec
- [ ] Subagent context optimization spec
