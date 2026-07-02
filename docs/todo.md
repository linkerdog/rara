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

- [x] Refactor ~7 large methods out of `impl TuiApp` to enable file split.
- [x] Restore Claude Code-style realtime transcript live-log writes from
      `push_entry` and clear the live log after turn commit so resume can
      recover partial turns after restart.
- [ ] Introduce an opencode-style configurable TUI theme token schema instead
      of static unused palette constants. Wire markdown, diff, syntax
      highlighting, picker, and overlay renderers through semantic theme tokens;
      for syntax highlighting, define how the app theme maps to or selects the
      active `syntect` theme.
- [x] Keep generic setup/list picker selected items visible when the selection
      moves past the first viewport.
- [x] Sidebar Plan replaces Todo (PR #597)
- [x] Approval dock above composer (PR #591, #594, #599)
- [x] Mouse text selection — drag-select + clipboard copy

## Context / Compaction

- [x] Model-aware context budget — CompactState.context_window_tokens per model
- [x] Compaction lifecycle — /compact command + PreCompact/PostCompact hooks (PR #598)
- [x] Tool result compression — ToolResultProjectionPolicy + model_preview_bash_output
- [x] Context file routing — FileSearchCandidateProvider → retrieval pipeline (spec-only PR #606)

## Agent / Subagent

- [x] Subagent token_budget field (PR #601)
- [ ] Subagent restart/reconnect — built-in capability, not a separate tool
- [x] Subagent context budget design — token_budget on AgentDefinition
- [x] Unify `.rara/agents` parsing for execution and `/status` discovery so
      `AgentDefinition` and `ImportedAgentProfile` cannot drift.
- [ ] Cache Claude-style agent definitions at runtime construction time and
      expose an explicit reload path instead of scanning on each `spawn_agent`.
- [ ] Implement remaining Claude-compatible `AgentDefinition` metadata:
      `token_budget`, `permission_mode`, `hidden`, and description/listing
      behavior.
- [ ] Add an end-to-end `spawn_agent` regression test proving custom
      `.rara/agents` definitions affect prompt body, tool filtering,
      `maxTurns`, and `planModeRequired`.

## Shared Task Lists

- [x] Add read-only `task_list` and `task_get` tools backed by
      `.rara/tasks/<task_list_id>/<task_id>.json`.
- [ ] Add write-side `task_create` and `task_update` tools with file locking,
      stale-read protection, owner/claim semantics, and dependency updates.
- [ ] Propagate task-list IDs through team and subagent runtime state so agents
      coordinate on the same shared task list without an explicit tool input.
- [ ] Add shared task watcher/TUI surfaces after mutation semantics are stable.

## Planning Control Plane

- [ ] Replace boolean plan approval handling with an explicit decision enum:
      approve, continue planning with feedback, and reject/cancel.
- [ ] Persist planning lifecycle state in the structured rollout log:
      `plan_ready`, `plan_revising`, `plan_approved`, and `plan_rejected`.
- [ ] Restore pending plan approval after restart and avoid reinjecting an
      approved-plan tool result more than once.
- [ ] Expose planning lifecycle fields in `/status` and `/context`: plan path,
      approval status, pending age, last decision, and approved plan revision.
- [ ] Support continue-planning feedback so rejecting a plan can carry user
      instructions back into planning mode instead of only a generic retry.

## Hooks

- [ ] Hook output injection into model context (blocked on sandbox policy)
- [ ] Hooks/plugin lifecycle spec

## Configuration

- [ ] Explicit embedding controls: enable/disable, provider override (low priority)
- [ ] `/status` context fields for model/provider/thread/retrieval/memory/workspace
- [ ] Decide the Gemini AI Studio runtime path: either wire `provider=gemini`
      to the native `GeminiBackend::new` API-key backend or remove that native
      API-key path in favor of the current OpenAI-compatible endpoint.
- [ ] Wire Codex model catalog refresh into the active TUI provider/model
      selection flow, or remove the catalog loader and stored
      `codex_model_options` state if Codex presets stay static.
- [ ] Wire Gemini Code Assist OAuth login into the TUI provider connection
      flow, or remove the Google OAuth task surface if Gemini remains API-key
      only in the terminal UI.

## Long-term

- [ ] Claude plugin runtime integration
- [ ] Control-plane readiness for new features
