# TODO

Active backlog only. Keep this file small and current.

## Provider Coverage

- [ ] Extend the [provider registry](features/provider-registry.md) with native
      Anthropic/Azure/Cohere adapters and per-provider tool/reasoning conformance
      fixtures before claiming full Rig parity.
- [ ] Add registry model discovery, credential removal, cross-provider
      `small_model`, and model variants. Preserve the documented configuration
      precedence and keep remote capability verification separate from key presence.

## Portable Provider Boundary

- [ ] Continue [#871](https://github.com/linkerdog/rara/issues/871) after the
      LLM/tool contracts and the [shared loop machine](features/portable-agent-loop.md):
      provider crates, portable context/tool effects, browser HTTP/SSE transport,
      host-compatible accounting clocks and future bounds, and browser runtime
      tests. The [contract](features/portable-llm-contracts.md) currently proves
      browser-target compilation only.

## Prefix Cache Cost Evaluation

- [ ] Before promoting session-stable tool schemas globally, extend the completed
      paired trials to the full runtime tool set and longer tasks with more
      repetitions. Refresh provider/model routing and prices for each run. See
      [the implementation checkpoint](journal/2026-09-11-prefix-cache-optimization.md).

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

- [ ] Complete [#921](https://github.com/linkerdog/rara/issues/921) transcript
      layout caching and bounded redraw work. Shared materialized visual rows
      fix the correctness boundary, not per-delta rendering complexity; see
      [the wrapping checkpoint](journal/2026-10-02-shared-transcript-wrapping.md)
      and [the scroll-anchor checkpoint](journal/2026-10-02-transcript-scroll-anchors.md).
      Frame scheduling and stable/mutable Markdown source caching are implemented
      checkpoints. Indexed shared committed blocks now remove complete-history
      clone/wrap/hash work from frames, scroll, and selection, with focused work
      counts and retained-allocation guards. Eligible active response bodies now
      retain stable visual blocks with a mutable preview and explicit replay
      epochs. Persistent balanced indexes bound retained root-handle copies
      logarithmically. Unchanged active-prefix assembly and comparison are now
      bypassed using mutation identities. Live thinking content and clock updates
      replace only their visible row block, preserving both surrounding sections.
      Other changed-input assembly/comparison and rewrapping remain explicit costs. Long
      mutable blocks and source-wide reference fallbacks still need work bounds;
      ordinary angle brackets now avoid control-cleanup replay, while completed
      legacy markers and complex control contexts retain explicit replay costs.
      See [the control-cleanup checkpoint](journal/2026-10-04-streaming-control-cleanup.md);
      exact-head CI/review/merge and terminal acceptance remain separate gates.
      Live thinking now copies only its selected four-row tail; its source
      parsing and long-row wrapping costs remain separate. See
      [the thinking checkpoint](journal/2026-10-03-bounded-thinking-window.md).
      See [the prefix checkpoint](journal/2026-10-03-active-prefix-cache.md).
      See [the thinking layout checkpoint](journal/2026-10-03-live-thinking-row-slot.md).
      See [the shared-row checkpoint](journal/2026-10-03-transcript-row-reuse.md).
      See [the active-stream checkpoint](journal/2026-10-03-active-stream-rows.md).
      Tables interrupting mutable paragraphs have a canonical-renderer check at
      every Unicode-safe two-chunk split, including CRLF and reference contexts.
      See [the source-cache checkpoint](journal/2026-10-03-incremental-markdown.md),
      [the streaming contract](features/streaming-transcript.md) and
      [the frame checkpoint](journal/2026-10-02-tui-frame-coalescing.md).
- [ ] Finish [#922](https://github.com/linkerdog/rara/issues/922) delivery gates:
      exact-head remote CI/review/merge and bounded real-terminal acceptance.
      Typed stop admission, execution-return terminal publication, matching-turn
      completion barriers, and stale-event fencing have focused automated proof.
      See [the cancellation checkpoint](journal/2026-10-03-turn-cancellation-barrier.md).
- [ ] Finish [#923](https://github.com/linkerdog/rara/issues/923) delivery gates:
      exact-head remote CI/review/merge and bounded real-terminal acceptance.
      Stateful display ingestion, invocation/stream-isolated bounded progress,
      safe paste/terminal previews, and same-length middle-edit selection have
      focused automated coverage. Legacy identity-free events cannot separate
      concurrent same-name calls. Assistant control-token cleanup now agrees
      across chunk boundaries, including delayed separators and DSML evidence;
      see [the control-cleanup checkpoint](journal/2026-10-04-streaming-control-cleanup.md).
      Explicit bidi controls now use visible code-point labels, while paste and
      submission retain source text and editing maps labels to source offsets.
      Joiners and emoji retain the existing visible-cluster policy; this does
      not detect general Unicode confusables or invisible payloads. See
      [the bidi checkpoint](journal/2026-10-04-bidi-display-annotations.md) and
      [the display checkpoint](journal/2026-10-03-display-text-boundary.md).
- [ ] Finish [#924](https://github.com/linkerdog/rara/issues/924) delivery gates:
      exact-head remote CI/review/merge and bounded real-terminal acceptance.
      Diagnostic/startup/path/title column clipping, whole-grapheme shared
      editing, normalized visual-row selection, and checked non-destructive
      goal restoration have focused automated coverage. See
      [the Unicode checkpoint](journal/2026-10-03-unicode-boundaries.md).
- [ ] Finish [#925](https://github.com/linkerdog/rara/issues/925): bounded
      interactive terminal acceptance, including tmux and macOS native resize
      reflow and stale frame fragments in scrollback. The vt100 backend does
      not model terminal-native reflow. Viewport
      ownership, shell handoff, and focus mode are covered by
      [the viewport checkpoint](journal/2026-10-03-inline-terminal-viewport.md).
      Ctrl-C/Ctrl-D quit ordering and Unix suspend/resume are covered by
      [the job-control checkpoint](journal/2026-10-03-tui-interrupt-suspend.md).
- [ ] Complete remaining keyboard contracts: resume-search cursor editing,
      Help General/Runtime scrolling, atomic large-paste placeholder editing,
      and an explicit opt-in Vim mode. Cache editor grapheme boundaries for
      repeated cursor reads on long drafts. See
      [input risks](interaction/composer-and-overlays.md#open-risks).
- [ ] Add a runtime-owned skill enablement update and readback before restoring
      editing controls to the read-only `/skills` inspector. See
      [CMD-05](interaction/commands.md#cmd-05-skill-inspection-does-not-pretend-to-change-runtime-policy).
- [ ] Extend [interaction quality verification](interaction/quality-verification.md)
      with the remaining narrow/CJK/paste surface matrix, presentation dependency
      checks, and physical terminal/multiplexer acceptance. Cancel/completion
      interleavings, vt100 output, isolated Unix PTY lifecycle checks, and TUI
      print/raw-color lints now have focused guards. Each new gate needs a
      concrete protected defect and RED evidence before becoming required.
      Item-level lint ownership, actual-loop quit/cancel, full-session shell
      handoff, and the real suspend input adapter are covered by
      [the review follow-up checkpoint](journal/2026-10-04-tui-review-followups.md).
- [x] Complete the session-scoped `RuntimeClient` migration: route typed
      commands and runtime snapshots/events through the client, then remove
      runtime registries and completion orchestration from `TuiApp`.
- [x] Move goal continuation, plan continuation decisions, rebuilt-agent
      continuity, and runtime persistence helpers out of TUI task logic.
- [x] Deliver RuntimeControlEvent directly to TUI task consumers and stop
      deriving runtime semantics from role/message transcript formatting.
- [x] Establish the `RuntimeClientPort` and `TuiController` seam without
      exposing `Agent`, registries, or task join handles to future fakes.
- [x] Route runtime receiver events and task completion through one
      `tokio::select!` mux, and redraw only after a visible state change.
- [x] Add a shared scripted `TuiHarness` and `FakeRuntimeClient` for
      snapshot, event, completion, cancel, disconnect, reconnect, and render
      lifecycle tests without wall-clock sleeps.
- [x] Route production TUI runtime projections and snapshots through the
      session-scoped in-process `RuntimeClientPort` adapter; drain the legacy
      task receiver without replaying events.
- [x] Replace the local `AgentEvent -> role/message -> parser` compatibility
      path with a typed TUI projection event carrying session and sequence
      identity.
- [x] Route interactive user prompts and session cancellation through the
      in-process `RuntimeClientPort` command mux.
- [x] Route plan, shell, and pending-input approval commands through the
      in-process `RuntimeClientPort` command mux.
- [x] Move compact, rebuild, and model-list execution behind
      `RuntimeClientPort` while preserving the existing in-process task
      lifecycle.
- [x] Remove `RuntimeClient`/`Agent` ownership from `TuiController` by moving
      task construction, completion, and runtime replacement access into the
      runtime command processor.
- [x] Remove mutable extension-registry projections from `TuiApp`; registry
      discovery and reload must remain runtime-owned while TUI receives typed
      snapshots. Production task construction now receives explicit runtime
      services; only test-only fixtures retain local registry setup helpers.
- [x] Construct `TuiController` directly from an injected port and add
      deterministic scripted lifecycle controls to the shared harness.

## Thread Goals

- [x] Implement [#931](https://github.com/linkerdog/rara/issues/931): explicit
      restore continues an eligible goal once idle; durable user-stop deferral,
      revision-checked admission, paused choice, summary, edit, replacement
      confirmation, and compact elapsed/budget status. See
      [thread goals](features/thread-goals.md) and the
      [implementation journal](journal/2026-10-03-goal-resume.md).
      Successful-turn usage remains charged; cancelled/error turns retain the
      existing non-ledger accounting policy.

## Memory Lifecycle

- [x] Own incremental session capture, compaction flush, fail-open warnings,
      stable append deduplication, and bounded shutdown drain in the runtime.
- [x] Load Nowledge Mem context at session start and refresh it after
      compaction through the built-in plugin instructions.
- [ ] Pre-fetch the Nowledge Mem Context Bundle / Working Memory into assembled
      context at session start (runtime-owned read), instead of relying on the
      model to invoke the MCP skills.

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
- [x] Keep volatile environment, mode, protocol/LSP, and retrieved-memory
      context out of the system prompt and preserve earlier model-visible
      request prefixes with typed append-only context.
- [x] Add an opt-in official DeepSeek API AB/BA harness with content-free
      request fingerprints, explicit cost gates, and hit/miss usage reporting.
- [ ] Run the official DeepSeek API AB/BA harness with an authorized credential and record
      `prompt_cache_hit_tokens` / `prompt_cache_miss_tokens` for comparable
      repeated turns; request-shape regressions alone do not prove a cache hit.

## Benchmark / Evaluation

- [x] Add a dynamic Harbor adapter that loads with
      `--agent rara_agent:RaraAgent` and invokes `rara exec --json` inside the
      benchmark task workspace.
- [x] Convert RARA JSONL events into full ATIF-compatible trajectory logs for
      Harbor result artifacts. See `docs/features/terminal-bench-evaluation.md`.
- [x] Run a Terminal-Bench 2.1 single-task smoke with the versioned
      `headless-coding-v1` profile and record the RARA revision, Harbor version,
      provider/model, run command, JSONL/ATIF artifacts, and official verifier
      reward. Process exit status alone is not a pass result.
- [ ] Re-run `terminal-bench/headless-terminal` with the evidence-delta
      verification-and-repair pass, musl binary, and active DeepSeek reasoning
      controls; confirm the reviewer starts with risks absent from the first
      summary, inspect both trajectories, and record the official verifier
      reward before treating the regression as closed.
- [ ] Expand the Terminal-Bench 2.1 smoke into a multi-task cohort and repeat
      selected tasks before reporting a suite-level score or comparing harness
      quality.

## Agent / Subagent

- [x] Replace process-global background-agent state with a session-tree-owned
      control, one shared active-child budget, and parent-scoped lifecycle tools.
- [x] Deliver background completion and inter-agent messages through ordered,
      bounded mailboxes at model-turn boundaries.
- [x] Keep multi-agent policy independent from reasoning effort and support
      per-invocation provider/model routing for every launch surface.
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
- [x] Route subagent `provider` / `model` targets through runtime backend
      assembly, including configured provider states and per-task `team_create`
      overrides.
- [x] Surface configured subagent provider/model targets in runtime status for
      agent definitions.
- [x] Define the default-deny policy for subagent plugin skill and MCP access.
- [x] Add a runtime-owned scoped plugin skill executor for explicit
      `AgentDefinition.pluginSkills` allowlists; child reload and discovery stay
      disabled. MCP and plugin memory authority remain parent-owned.
- [ ] Evaluate a structured runtime agent catalog before adding
      description-driven automatic selection for built-in and custom profiles.
- [ ] Decide whether `team_create` should accept named agent definitions in
      addition to the built-in `general`, `explore`, and `plan` task kinds.
- [ ] Add bounded recursive delegation with an explicit depth budget after
      child-visible orchestration permissions are specified.
- [ ] Add a pairing-preserving, token-budgeted parent-history projection before
      exposing subagent context fork modes.
- [ ] Add proactive-delegation evaluations for unnecessary delegation,
      synthesis quality, latency, and token amplification.

## Embedded Runtime

- [x] Expose the root package as a Rust library with a workspace-scoped
      `EmbeddedRuntime`, typed events, isolated state roots, and agent control.
- [x] Add host-controlled backend and tool injection, with explicit opt-out
      from ambient extension discovery, RARA-owned memory facilities, and
      local transcript checkpoints.
- [x] Add stable host session IDs plus transcript hydration, replacement,
      completed-turn handoff, and failure evidence.
- [ ] Add async transcript and context store traits, then continue the
      `rara-agent` / control-plane crate split.

## Runtime Session

- [x] Add the library-first `RuntimeSession` actor, bounded commands, typed
      turn handles, cancellation, snapshots, replay, and explicit shutdown.
- [x] Move ACP, Wire, print, exec, ask, and embedded execution onto the session
      handle and add a non-global multi-session `RuntimeHost`.
- [ ] Move the TUI rebuild, approval, goal, and maintenance pipeline from its
      compatibility `RuntimeClient` owner into `RuntimeSession` commands.
- [ ] Extract the minimal runtime dependency graph so external Rust hosts do
      not pull TUI, local-model, ACP, or OAuth implementations
      ([#860](https://github.com/linkerdog/rara/issues/860)). The Git fixture
      validates portable contracts and the shared asynchronous executor; extend
      it to portable context/tool adapters and the
      real `RuntimeSession` with cancellation and transcript readback before
      closing the issue. Follow the [extraction sequence](features/portable-tool-contracts.md#runtime-extraction-sequence).
- [ ] Add a Nowledge Mem compatibility harness for provider, tool, event,
      transcript, usage, cancellation, and MCP parity before replacing Rig in
      production lanes.

## App Server Stdio

- [ ] Add durable resume, root discovery or additional control families only with
      independent ownership, bounds and recovery evidence. The version1 CLI now
      explicitly rejects these unsupported methods. See
      [the protocol contract](features/app-server-stdio.md) and
      [the implementation journal](journal/2026-09-18-app-server-stdio.md).

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

- [x] Remove the bundled local embedding sidecar, the provider/local embedding
      overrides, and the deprecated `local_embeddings` config field entirely.

## WASM Core

- [ ] Add `wasm32-unknown-unknown` CI once the Rust target is installed in CI.
- [ ] Add a JavaScript binding package around `rara-wasm-core` when a browser
      client is ready to consume it.

## Provider Catalog

- [x] Enrich connected DeepSeek/Kimi model lists with static and API-provided
      context-window metadata and show it in model pickers.
- [ ] Move provider availability and authentication-method projection from the
      TUI compatibility adapter into the session-scoped runtime control plane;
      see `docs/features/provider-connection-redesign.md`.

## Patch Engine

- [x] Add a typed `PatchAction` preview API for approval UI and app-server
      surfaces.
- [x] Add structured applied-delta failure reporting for partial filesystem
      writes.

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
- [x] Claude plugin extension registries for skill invocation/reload and agents.
- [x] Control-plane readiness for new features.
