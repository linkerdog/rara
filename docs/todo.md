# TODO

## Recently Completed (2026-05-20)

- [x] TUI popup color-block redesign: replace wireframe Borders with panel bg (#455)
- [x] TUI popup adaptive sizing: popup_rect with overflow clamp (#455)
- [x] Approval/stderr readability: remove special backgrounds (#459)
- [x] wrapped_text_height fix: account for block padding (#459)

Active backlog only. Keep this file small and current.

## Execution Plan (2026-06-14)

1. Finish plugin/runtime status correctness before adding more plugin surface.
2. Add `rara plugin install/list/remove` once runtime registration is observable. (done)
3. Improve TUI live feedback: thinking collapse/summary (✅ done #590), live bash transcript (✅ already streaming via ToolProgressEvent).
4. Close context/embedding correctness: project_context merge (✅ done #589), canonical vector schema,
   model-aware budgeting.
5. Split P0 large files only after the behavior surfaces above have tests.

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
- [x] background-tasks extraction: moved run_background_bash_task, read_stream_chunks, kill_child_process_group to rara-background-tasks crate (bash.rs 2315→1859 lines)

## Next Up (short-term, ready to implement)

### Memory (Phase 2–4)
- [x] `summary.md` index: auto-update on session file writes, 5KB retention cap (#442)
- [x] `search_memory`: rg + LanceDB merged search with native fallback (#442)
- [x] Context injection: load summary.md into system context every turn (#442)
- [x] Wire session file creation into runtime startup (#442)
- [x] Claude-style one-line pointer index format (#442)
- [x] Concurrent-safe writes: atomic temp-file + fs2 locking (#442)
- [x] Codex-inspired read-path template with usage instructions (#442)
- [x] Migrate `memory_files` to `rara-memory` crate (#442)
- [x] CC-style consolidation: scheduler + lock + subagent dispatch (#537)

- [x] Consolidation tool restriction: documented scope (#563)
- [x] Consolidation DreamTask UI: /dream command + status (#564)
- [x] Consolidation inline message (#552)
- [x] search_memory tool registration (#553)
- [x] MemoryQuery hook lifecycle + SearchMemoryTool callback slot (#559)

### Hooks
- [x] File-based hook discovery + context injection (#442)
- [x] File-hooks lifecycle spec (`docs/features/file-hooks.md`) (#442)
- [x] Command hook execution: `run_command_hook` + `make_command_hook` (#444)
- [x] Hook runtime startup + event bus subscription (#444)
- [x] Per-phase conditional hook injection (#556)
- [x] PreToolUse hook (#556)
- [ ] Hook output injection into model context (blocked on sandbox policy)
- [x] Wire plugin hook registration into runtime startup/status panel

### LSP (Phase 1)
- [x] LSP runtime wiring: shared manager, lazy server startup, diagnostics parsing, sidebar status
- [x] `lsp_diagnostics` tool: return cached diagnostics for a file
- [x] Replace sleep-based LSP initialization with response-aware handshake handling

### File Splits (P0)
- [ ] `tools/bash.rs` (1859 lines) → by tool group (partial: background-tasks extracted)
- [ ] `tools/pty.rs` (1649 lines) → stream read/write
- [ ] `tools/agent.rs` (1701 lines) → by tool group
- [ ] `memory_store.rs` (1625 lines) → LanceDB + legacy
- [ ] `thread_store.rs` (1568 lines) → read/write separation
- [ ] `agent.rs` (1339 lines) → tool/plan/history
- [ ] `tui/render.rs` (990 lines) → move to existing submodules
- [x] context/assembler.rs → assembler/mod.rs directory (#561)

- [x] `runtime_control.rs`: add per-item comments to 22 `#[allow(dead_code)]` ACP types (#547)
- [x] `hook_registry.rs`: remove `#[allow(dead_code)]` from `all_hooks` (#532)
- [x] `google_oauth.rs`: documented as technical debt — 21 items to delete after OAuth migration (#551)
- [x] `theme.rs`: remove module-level `#![allow]`, add per-item `// Nord palette` comments (#545)
- [x] `mcp_status.rs`: add scaffolding comment per AGENTS.md (#547)
- [x] `tui/custom_terminal.rs`: add `#[allow(deprecated)]` for Cell::skip (#549)
### Features
- [x] Embedding dimension consistency: detect + rebuild on mismatch
- [x] Sidebar status update: `app.local_model_server` after bootstrap

### Specs to Write
- [x] Hooks/plugin lifecycle spec — `docs/features/file-hooks.md`
- [x] Subagent context optimization spec — covered by project_context design
- [x] Compaction as explicit runtime lifecycle event — `/compact` command + PreCompact/PostCompact hook phases
- [ ] Model-aware context budget — token limits per model window

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
- [x] Fix plugin lifecycle parity: SessionEnd mapping added (#568. SessionEnd=9 last)
- [x] Add `rara plugin install/list/remove` CLI commands.

## TUI / Composer / Status

- [x] BottomPaneModel struct with structured notice/prompt/input.
- [x] Finish BottomPaneModel migration: composer/sizing already reads from model
- [ ] Thinking expand/collapse with duration summary.
- [ ] Live bash transcript: streaming output frames.
- [ ] After local embedding sidecar lands: `/status` context fields for backend/model/state.
- [x] Plugin hook registration feed into TUI runtime status panel.

## Context / Embeddings

- [x] Record canonical embedding dimension/schema version next to each vector store.
- [x] Auto-rebuild vector store on dimension mismatch.
- [ ] Make context budgeting model-aware instead of one fixed heuristic.
- [ ] Turn compaction into an explicit runtime lifecycle event.
- [ ] Read project-level AGENTS.md → `project_memory` → inject into context.

## Memory

- [x] Implement session/global memory files with summary-driven retrieval (Phase 2 of spec).
- [x] Wire `memory_summary` summary index into context assembler.
- [x] Concurrent-safe writes: atomic temp-file + fs2 locking for shared memory files.
- [x] CLI: `rara resume <THREAD_ID>`, `rara thread <THREAD_ID>`, `rara threads` (#537)

## Model Support

- [x] uv venv --python 3.14 --seed for managed Python venv.
- [ ] Add explicit embedding controls: enable/disable, provider override.
- [x] Claude-style inline `/model` command surface for runtime switching.
- [ ] Add `/status` context fields for model/provider/thread/retrieval/memory/workspace.

## Agent / Tools

- [ ] Refactor ~7 large methods out of `impl TuiApp` to enable file split.
- [ ] Design subagent context budget as a first-class property.
- [ ] Subagent restart/reconnect semantics.
- [ ] Tool result compression — auto-truncate long outputs with summary, align with Claude Code.
- [ ] Claude plugin runtime integration (long-term).
- [x] `runtime_control.rs`: add per-item comments to 22 `#[allow(dead_code)]` ACP types
- [x] `google_oauth.rs`: add FIXME comment documenting superseded-by-codex-login status
- [x] `mcp_status.rs`: add comment to unused enum variants
- [x] `file_search_provider.rs`: add scaffolding comment per AGENTS.md
- [x] `acp_consumer.rs`: add scaffolding comment per AGENTS.md

## Spec Hygiene

- [x] Memory spec: `docs/features/session-global-memory.md`
- [x] LSP spec: `docs/features/lsp-integration.md`
- [ ] Hooks/plugin lifecycle spec
