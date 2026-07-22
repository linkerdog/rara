# TODO

Active backlog only. Keep this file small and current.

## Execution Plan (2026-08-28)

1. ✅ Plugin/runtime status correctness.
2. ✅ `rara plugin install/list/remove`.
3. ✅ TUI live feedback: thinking collapse + live bash transcript.
4. ✅ Context/embedding: project_context merge, canonical vector schema, model-aware budgeting.
5. ✅ P0 file splits completed.

## P0 File Splits

- [x] `tools/bash.rs` — tests extracted to bash_tests.rs (914 + 947 lines)
- [x] `tools/agent.rs` — agent_def extracted via include! (1749→1551, PR #603)
- [x] `tools/pty.rs` — split into real submodules (15-line facade; largest child 617 lines)
- [x] `tui/runtime/tasks.rs` — completion orchestration split into `tasks/completion.rs` (747 + 545 lines)

## TUI / UX

- [x] Keep `rara-file-search` as the shared backend for TUI file suggestions
      and `list_files`; keep automatic retrieval as optional low-priority
      paths-only `RetrievalCandidate` / `MemorySelection` input.
- [x] Refactor ~7 large methods out of `impl TuiApp` to enable file split.
- [x] Restore Claude Code-style realtime transcript live-log writes from
      `push_entry` and clear the live log after turn commit so resume can
      recover partial turns after restart.
- [x] Introduce an opencode-style configurable TUI theme token schema instead
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

## Benchmark / Evaluation

- [x] Add a dynamic Harbor adapter that loads with
      `--agent rara_agent:RaraAgent` and invokes `rara exec --json` inside the
      benchmark task workspace.
- [x] Convert RARA JSONL events into full ATIF-compatible trajectory logs for
      Harbor result artifacts. See `docs/features/terminal-bench-evaluation.md`.

## Agent / Subagent

- [x] Subagent token_budget field (PR #601)
- [x] Subagent restart/reconnect — built-in capability, not a separate tool
- [x] Subagent context budget design — token_budget on AgentDefinition
- [x] Add shared TaskList/TaskGet runtime tools backed by a `.rara/tasks/<task_list_id>/`
      task store so teams/subagents can coordinate beyond session-local `todo_write`.
- [x] Unify `.rara/agents` parsing for execution and `/status` discovery so
      `AgentDefinition` and `ImportedAgentProfile` cannot drift.
- [x] Cache Claude-style agent definitions at runtime construction time and
      refresh them through runtime rebuild instead of scanning on each
      `spawn_agent`.
- [x] Apply Claude-compatible `hidden` and `description` metadata to
      repo-local agent listing/status behavior.
- [x] Apply Claude-compatible `AgentDefinition.permission_mode` to subagent
      execution policy.
- [x] Implement remaining Claude-compatible `AgentDefinition` execution
      metadata: `token_budget`.
- [x] Add an end-to-end `spawn_agent` regression test proving custom
      `.rara/agents` definitions affect prompt body, tool filtering,
      `maxTurns`, and `planModeRequired`.

## Shared Task Lists

- [x] Add read-only `task_list` and `task_get` tools backed by
      `.rara/tasks/<task_list_id>/<task_id>.json`.
- [x] Add `task_create` with file locking and atomic pending-task writes.
- [x] Add `task_update` with field, status, metadata, dependency, and delete
      mutations under the task-list lock.
- [x] Add revision or timestamp based stale-read protection for `task_update`.
- [x] Add owner/claim semantics that reject conflicting concurrent claims.
- [x] Propagate task-list IDs through team and subagent runtime state so agents
      coordinate on the same shared task list without an explicit tool input.
- [x] Add snapshot-backed shared task status and TUI surfaces after mutation
      semantics are stable.
- [x] Add a live filesystem watcher for shared task files if cross-process task
      changes need to update the TUI without a new runtime snapshot.
- [x] Add a user-facing command for switching the active shared task list during
      a TUI session.

## Planning Control Plane

- [x] Replace boolean plan approval handling with an explicit decision enum:
      approve, continue planning with feedback, and reject/cancel.
- [x] Persist planning lifecycle state in the structured rollout log:
      `plan_ready`, `plan_revising`, `plan_approved`, and `plan_rejected`.
- [x] Restore pending plan approval after restart and avoid reinjecting an
      approved-plan tool result more than once.
- [x] Expose planning lifecycle fields in `/status` and `/context`: plan path,
      approval status, pending age, last decision, and approved plan revision.
- [x] Support continue-planning feedback so rejecting a plan can carry user
      instructions back into planning mode instead of only a generic retry.
- [x] Persist plan submission timestamps and approved plan hashes so `/status`
      and `/context` can render concrete pending age and approved revision
      values instead of `-`.

## Hooks

- [x] Hook output injection into model context; command hook execution remains
      constrained by sandbox policy.
- [x] Hooks/plugin lifecycle spec

## Configuration

- [x] Default bundled local embedding sidecar startup to off while preserving
      explicit `local_embeddings = "auto"` opt-in.
- [ ] Explicit embedding provider override beyond `off` / `auto` (low priority)

## ACP Compatibility

- [x] Replace the single ACP `active_agent` with session-scoped runtime state;
      bind each `session/new` ID to its own history and requested workspace cwd.
- [x] Route ACP cancellation by `CancelNotification.session_id` and retain the
      session ID in control-plane provenance so one client cannot interrupt
      another session.
- [x] Map ACP output from structured runtime events rather than presentation
      text: include thinking, tool lifecycle and streams, approvals, plans,
      todos, warnings, errors, cancellation, and completion.
- [x] Add focused ACP regressions for multi-session isolation, cwd propagation,
      session-targeted cancellation, and structured event translation.

## Long-term

- [x] Claude plugin discovery source metadata and ordered de-duplication.
- [x] Claude plugin TUI runtime startup across user and project plugin directories.
- [x] Claude plugin runtime startup parity for headless, ACP, and Wire surfaces.
- [x] Claude plugin explicit plugin directory CLI surface for TUI sessions.
- [x] Claude plugin explicit plugin directory config persistence.
- [x] Claude plugin matcher evaluation for tool hooks.
- [x] Claude plugin `SessionEnd` command hook dispatch.
- [x] Claude plugin skill directory prompt summaries.
- [x] Claude plugin non-tool command hook dispatch for `SessionStart` and `UserPromptSubmit`.
- [x] Claude plugin lifecycle parity: structured hook output observability.
- [x] Claude plugin `.mcp.json` extension registry integration.
- [x] Claude plugin command extension registry summaries.
- [ ] Claude plugin extension registries for skill invocation/reload and agents.
- [ ] Control-plane readiness for new features
