# TODO

Active backlog only. Keep this file small and current.

## Execution Plan (2026-08-28)

1. ✅ Plugin/runtime status correctness.
2. ✅ `rara plugin install/list/remove`.
3. ✅ TUI live feedback: thinking collapse + live bash transcript.
4. ✅ Context/embedding: project_context merge, canonical vector schema, model-aware budgeting.
5. 🔵 P0 file splits stalled — pty blocked by nested test modules; agent reduced via PR #603.

## P0 File Splits

- [x] `tools/bash.rs` — tests extracted to bash_tests.rs (914 + 947 lines)
- [x] `tools/agent.rs` — agent_def extracted via include! (1749→1551, PR #603)
- [ ] `tools/pty.rs` (1649) — blocked: nested `mod tests` incompatible with `#[path]` and `include!`
- [ ] `memory_store.rs` (1625)
- [ ] `thread_store.rs` (1568)

## TUI / UX

- [ ] Refactor ~7 large methods out of `impl TuiApp` to enable file split.
- [x] Sidebar Plan replaces Todo (PR #597)
- [x] Approval dock above composer (PR #591, #594, #599)
- [x] Mouse text selection — drag-select + clipboard copy

## Context / Compaction

- [x] Model-aware context budget — CompactState.context_window_tokens per model
- [x] Compaction lifecycle — /compact command + PreCompact/PostCompact hooks (PR #598)
- [x] Tool result compression — ToolResultProjectionPolicy + model_preview_bash_output
- [x] Context file routing — FileSearchCandidateProvider → retrieval pipeline (spec-only PR #606)

## Agent / Subagent

- [x] Agent team mode — team_create, sub_agent_list/stop/resume, StructuredResult aggregation

- [x] Subagent token_budget field (PR #601)
- [x] Subagent restart/reconnect — sub_agent_resume with parent-sidechain replay
- [x] Subagent context budget design — token_budget on AgentDefinition

## Hooks

- [ ] Hook output injection into model context (blocked on sandbox policy)
- [ ] Hooks/plugin lifecycle spec

## Configuration

- [ ] Explicit embedding controls: enable/disable, provider override (low priority)
- [ ] `/status` context fields for model/provider/thread/retrieval/memory/workspace

## Long-term

- [ ] Claude plugin runtime integration
- [ ] Control-plane readiness for new features
