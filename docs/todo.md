# TODO

Active backlog only. Keep this file small and current.

## Suggested Rollout Order

1. Claude plugin runtime integration and extension-source unification
2. Provider/model connection polish and `reasoning_summary` completion
3. TUI bottom-pane/view-stack cleanup and transcript rendering parity
4. Web/source-reporting and auxiliary-model routing
5. Cross-process sub-agent durability and Terminal-Bench readiness
6. Security, sandbox policy provenance, and secret handling
7. Release distribution and package-manager adapters
8. Oversized-module cleanup and docs/spec hygiene

## Runtime Control Plane / ACP / Wire

- [x] P0+: Add a runtime input-control bridge so appserver/ACP/Wire can submit prompts, follow-ups, pending-input answers, approval decisions, and cancellation intents through `RuntimeControlRequest::Input` / `SessionControlRequest` instead of TUI-only handlers (merged #338+).
- [x] Define adapter-neutral runtime control request/event types for ACP, Wire, TUI, CLI, and future appserver entrypoints (see `docs/features/runtime-control-plane.md`).
- [x] Add Claude-style `todo_write` runtime state with session persistence, TUI update cards, and structured Wire/ACP-ready events.
- [x] Add source-aware MCP config registry for user `config.toml` and project `.mcp.json` with duplicate-name conflict failure.
- [x] Route ACP prompt/cancel/session handling through the normal RARA runtime path instead of the current stub (merged #338+).
- [x] Add protocol subscriber plumbing on top of the structured `AgentEvent` runtime-control bridge.
- [x] Add MCP connection manager status model (`configured`, `connecting`, `connected`, `refreshing`, `reconnecting`, `failed`, `disabled`) from `McpRegistry`.
- [x] Add `/mcp` status surface grouped by scope and source path.
- [x] Publish `/mcp` status snapshots as structured runtime events for future ACP/Wire/appserver subscribers.
- [x] Add dynamic MCP tool/resource/prompt refresh through structured runtime events (scaffold via McpConnectionManager).
- [x] Add bounded MCP auto-reconnect with manual reconnect command (scaffold via McpConnectionManager).
- [x] Add MCP resource references as source objects visible in `/context`.
- [x] Add MCP Tool Search so large MCP tool sets are discovered on demand instead of injected into every prompt.
- [x] Support protocol-registered prompt sources with provenance, scope, budget hints, and `/context` visibility (scaffold via protocol_sources.rs).
- [x] Support protocol-registered skill sources through the same `SkillRegistry` precedence and override reporting as local skills (scaffold via protocol_sources.rs).
- [x] Add protocol-safe memory mutation/query scaffolding that creates memory records and selection views without bypassing `MemorySelection` (scaffold via protocol_sources.rs).
- [x] Add hook declaration scaffolding for protocol and repo extensions; keep execution disabled until permission and sandbox policy are explicit.
- [ ] Ensure every new skill, memory, prompt, hook, planning, approval, and output feature is control-plane-ready rather than TUI-only.
- [x] Add a `support-acp` integration skill for IDE and third-party app authors, covering ACP startup, runtime-control input intents, output event subscription, cancellation/preemption, approvals, MCP/tool-search expectations, and safe context-source registration (see `docs/features/support-acp-integration.md`).

## Plugins / Extension Runtime

- [x] Add the first `rara-plugins` crate for Claude Code plugin discovery, `plugin.json` parsing, `hooks/hooks.json` parsing, command-hook execution, timeouts, and tests.
- [ ] Wire plugin hook registration into runtime startup so discovered command hooks actually fire through `HookRuntime` for TUI, CLI, ACP, and Wire entrypoints.
- [ ] Fix plugin lifecycle parity gaps before broader rollout: `SessionEnd` mapping, matcher evaluation, hook stdout/stderr observability, and blocking `{ "continue": false }` semantics.
- [ ] Add `rara plugin install/list/remove` with local-path and git-source support, plus explicit trust/sandbox copy.
- [ ] Parse plugin `.mcp.json` into the existing MCP registry and launch lifecycle instead of leaving MCP config as inert metadata.
- [ ] Register plugin-provided commands, skills, and agents as structured extension sources with precedence and `/context` visibility.
- [ ] Design prompt/http/agent hook support only after command hooks have runtime integration, observability, and permission boundaries.

## Configuration / Provider Surface

- [ ] Complete `reasoning_summary` rollout across backend requests, switching flows, and status surfaces; retire remaining `thinking`-only behavior outside migration fallback.
- [ ] Surface provider-scoped reasoning configuration in `/status` and provider/model switching flows.
- [ ] Study Gemini/Codex-style multi-model routing for top-tier + flash/fast model pairing.
- [ ] Deepen provider-surface continuity after hot-swap: auth-mode/endpoint alignment, provenance reporting.
- [ ] Align Codex endpoint selection with auth mode (ChatGPT/Codex login vs API key).
- [ ] Show provider-catalog context windows in ModelSearch items for DeepSeek/Kimi/OpenAI/Codex where known.
- [ ] Load model lists from provider APIs for connected providers, with provider-catalog windows as fallback metadata.
- [ ] Split Codex-specific persisted auth/config to `~/.codex`, keep RARA config under `~/.rara`.

## Workspace / Skills / Prompt Sources

- [ ] Add session-style incremental file search for TUI file pickers and context file routing on top of `crates/file-search`.
- [x] Tests for workspace prompt-source discovery and cache invalidation (cwd changes, git branches, nested workspaces).
- [x] Define `WorkspaceMemory` cache invalidation rules for prompt files and environment info (see `docs/features/workspace-memory-cache.md`).
- [x] Unify `discover_prompt_sources()` and TUI `/status` source reporting.
- [x] Add directory-walking `.rara/rules/*.md` prompt sources from CWD to repo root.
- [ ] Define and implement `.rara/local.md` semantics, scope, precedence, and visibility before enabling it as a prompt source.
- [ ] Continue the Codex/Claude Code prompt audit after the first Claude-derived slice: skill invocation and compact-summary continuation contracts are migrated; remaining candidates are plan-mode phase discipline and dynamic environment/status snapshots.
- [ ] New prompt inputs through structured source objects, `MemorySelection`, lifecycle events, and runtime-control provenance — not ad hoc text. Protocol prompt sources now retain provenance, convert into prompt-runtime sources, atomically snapshot from the live registry at the user-query boundary, and emit registered/injected/dropped lifecycle events; next slice should extend the same bridge to protocol skill/hook visibility.
- [ ] Project-scoped extension surface for `.claude/agents/`, `.claude/hooks/`, `.agents/skills/` with precedence rules.
- [x] Port reusable AgentHub `.agents/skills` patterns into RARA repo skills: docs journal/spec writing, project title rules, and focused testing guidance; do not copy AgentHub-specific ACP rendering/debug skills directly.
- [ ] Claude-style `verify` skill and `verifier-*` convention (see `docs/features/verify-skill.md`).
- [ ] Evolve `SkillTool` to Codex/Claude contract (see `docs/features/skill-tool.md`): frontmatter, scopes, override visibility.
- [ ] Surface skill precedence/override across home, repo, nested roots.

## Web Tools

- [ ] Replace `web_fetch` HTML-to-text with higher-fidelity markdown conversion.
- [ ] Add capability-aware web prompt injection and tool registration so `/status`, `/context`, ACP, and Wire can show whether web search is disabled, anonymous Exa-backed, authenticated Exa-backed, or provider-native.
- [ ] Add source-reporting enforcement for web-backed answers: capture source URLs from `web_search` / `web_fetch` and surface them to the final-response path.
- [ ] Add current-date query hints for recent/current web searches so models do not search with stale years.
- [ ] Add first-class domain allow/block filters and bounded per-turn search budget to the RARA `web_search` tool schema.
- [ ] Add structured web-search runtime events with query, provider, source URLs, truncation state, and source-reporting readiness for `/context`, `/status`, ACP, and Wire.
- [ ] Study auxiliary-model execution for search-only subqueries while preserving source reporting and prompt-injection boundaries.
- [ ] Add provider-native web search mode support for compatible OpenAI Responses-style backends, with local Exa-backed search as the portable fallback.

## Memory / Retrieval / Persistence

- [x] Add `MemoryRecord` runtime model with title, Markdown content, labels, importance, timestamps, source, and scope.
- [x] Introduce `MemoryStore` as the memory-domain façade over the current LanceDB-backed `VectorDB`.
- [x] Turn `remember_experience` and `retrieve_experience` into compatibility adapters over `MemoryStore`.
- [x] Persist full `MemoryRecord` domain records with session id, thread id, and source span provenance.
- [x] Replace `retrieve_session_context` stub with LanceDB hybrid search over conversation checkpoints.
- [x] Add backend `ThreadStore` APIs for markdown export and summary-to-memory distillation.
- [x] Promote LanceDB-backed retrieval from `MemoryStore` into ranked `MemorySelection` candidates.
- [x] Add pinned/retention policy so pinned, user-created, and high-importance memories are excluded from automatic cleanup.
- [x] Add memory update/delete/list-label control-plane scaffolding for ACP/Wire without exposing LanceDB APIs.
- [x] Upgrade thread distillation from summary capture to LLM-assisted 2-8 record extraction with deduplication.
- [x] Move raw session checkpoints into per-session append shards instead of the global LanceDB `conversations` table.
- [x] Promote `rollouts/<session_id>/transcript.jsonl` from compatibility mirror to canonical model-history source.
- [x] Wire foreground sub-agent tools to write parent-scoped sidechain transcripts under `rollouts/<parent_session_id>/subagents/`.
- [x] Add append-only parent/child spawn-edge rollout metadata for foreground sub-agent calls.
- [x] Index parent/child spawn-edge metadata in `StateDb` for resume/listing queries.
- [x] Add in-process background sub-agent resume/stop over the sidechain transcript contract.
- [x] Add an explicit promotion API from session context shards into global `MemoryRecord`s.
- [x] Add scheduler/policy gates for periodic session-shard promotion so background writes are opt-in and observable.
- [x] Durable in-turn checkpoints: persist after each message/tool-result batch, atomic writes, crash-tolerant `SessionManager`.
- [x] Auto-memory extraction: background LLM-driven fact extraction after every 5 turns, inserted into LanceDB via MemoryStore (PR #375).
- [x] Directory-walking rules layer: `.rara/rules/*.md` discovered from CWD to repo root (PR #375).
- [ ] Prototype local embedding models for memory retrieval, comparing local inference quality/latency against the current remote or fallback embedding path before choosing a durable backend.
- [ ] Add auto-memory extraction controls and observability: enable/disable config, last-run status, error reporting, dedupe metrics, and stale/timeout diagnostics.
- [ ] Define cross-process background sub-agent restart/reattach semantics.
- [x] Compaction as first-class lifecycle event: persist summaries, token counters, metadata ownership.
- [x] Add prompt-too-long retry for compaction by dropping oldest API-round groups.
- [x] Add partial compact support around a selected message boundary (`from` / `up_to`).
- [x] Add post-compact source descriptors and surface them in `/context` and `/resume`.
- [x] Add generic memory/hook/skill/MCP carry-over consumer shape that validates stable compaction source descriptors.
- [x] Add concrete retrieved-memory carry-over producer.
- [x] Add concrete invoked-skill carry-over producer.
- [x] Add concrete hook/MCP carry-over producers.
- [x] Add prefix-stable tool-result projection before model requests.
- [x] Add per-request microcompact observability to `/context`.
- [x] Add an OTEL-ready context observability event model for compaction, microcompact projection, cache usage, and memory retrieval.
- [x] Add terminal environment detection for TUI compatibility, diagnostics, and future OTEL attributes.
- [x] Surface terminal metadata in `/status` diagnostics and future OTEL attributes.
- [x] Add provider-gated cache-edit microcompact only for backends that explicitly declare cache-edit support.
- [x] Surface main model vs auxiliary model routing in `/status` and `/context`.
- [ ] After context observability is complete, add an auxiliary-model compression hook for retrieval candidates without changing durable memory records.
- [x] `ThreadStore` / `ThreadRecorder`: from façade over `SessionManager`+`StateDb` to true structured thread store.
- [x] Thread-scoped and workspace-scoped `MemoryRecord` storage with promotion rules.
- [x] Initial retrieval orchestration layer from `docs/features/retrieval-orchestration.md`: typed candidates, orchestration view, richer `/context`, deterministic dedupe, and ACP/Wire event exposure.
- [x] Add the first `RetrievalSourceProvider` boundary for current memory, session, thread-history, vector-slot, tool-result, and file-search candidates.
- [x] Add `RetrievalSourceProvider` implementations for hook output and graph sources after the current memory/session/file/MCP path is stable.
- [x] Initialize LanceDB and wire FTS/vector/hybrid search paths behind the existing memory index façade.
- [x] Make memory mutation/query control-plane-ready so ACP/Wire can inspect and add memory without directly editing prompt text.

## TUI / Transcript

- [x] Remove crossterm history write path, unify all rendering through Ratatui (PR #272).
- [x] Decouple overlays from transcript layout (pure top layer, no viewport perturbation).
- [x] Split the bottom pane into composable activity, composer, queued-preview, and footer modules (PR #366).
- [ ] Complete `BottomPaneModel` migration: activity/footer now use structured view data, but composer and sizing still read broad `TuiApp` state directly.
- [ ] Move approval, request-input, command-palette, and picker flows toward a Codex-style bottom-pane view stack after the rendering split is stable.
- [ ] Post-exit resume hint (e.g. `rara resume --last`).
- [x] Claude-style repo context hints beneath input area (GitHub PR link).
- [x] Codex/Claude-style transcript role cards (`You` / `Agent` / `System`).
- [ ] Stabilize active response blocks while streaming, avoid generic transcript fallback.
- [ ] Rework built-in command TUI (`/help`, `/model`, `/status`, command palette, overlays) to match Codex/Claude.
- [ ] Refine `/status`: provider/model state, reasoning, sandbox/network, context injection, tool availability.
- [x] Tool-action summaries more source-aware and file-aware.
- [ ] Live `bash` transcript: lifecycle framing, streamed stdout/stderr, long-output folding.
- [ ] High-fidelity render pass for `write/update`, inline diffs, approval cards, message-card hierarchy.
- [ ] Add committed thinking expand/collapse interaction and elapsed-time summary after the first collapsible thinking display slice.
- [ ] Strengthen terminal Markdown rendering parity: GitHub-flavored Markdown coverage, local file-link rendering, fenced code blocks, list wrapping, and focused snapshot coverage.
- [ ] Expand TUI snapshot coverage.
- [ ] Keep transcript and pending-interaction state backed by structured events that ACP/Wire output subscribers can reuse.
- [x] Add first-class todo sections to `/context` and `/status` from `TodoContextView`.

## Security / Reliability / Performance

- [x] Structured command model (`program`, `args`, `cwd`, `allow_net`) in `src/tools/bash.rs` and `crates/sandbox`.
- [x] Classifier and routing model from `docs/features/classifier-and-routing.md`.
- [ ] Auditable permission + sandbox-bypass rules (Codex/Claude-inspired);
      segment-level bash prefix reuse is implemented, but typed approval-policy
      modes and full rule provenance remain open.
- [x] Structured auto-permission classifier with compact transcript projection.
- [x] Background-task state classifier: `working` / `blocked` / `done` / `failed`.
- [ ] `secrecy::SecretString` end-to-end for API keys, audit error paths.
- [ ] Replace `.expect(...)` with structured `anyhow::Context` errors.
- [ ] Review path/command validation in `bash`, file tools, sandbox.
- [ ] Rework token accounting in `src/agent.rs` (avoid re-encoding full history).
- [ ] Replace fixed 100ms TUI event polling loop.

## Evaluation / Benchmarks

- [ ] Terminal-Bench readiness: add a headless adapter, preserve structured trajectories, and start with a small smoke run that records RARA revision, dataset version, provider/model, sandbox mode, and failure taxonomy.

## Release / Distribution

- [x] Add tag-driven GitHub Release workflow for `rara` binary archives and checksums.
- [x] Add release matrix smoke test that unpacks each archive and runs `rara --version`.
- [ ] Add npm package layout and staging script for meta-package plus platform packages.
- [ ] Add npm packaging tests and trusted-publishing workflow step.
- [ ] Decide Homebrew tap ownership and add formula update workflow consuming GitHub Release checksums.

## Code Organization / Docs

- [x] Add `TuiMaintainer` to event loop (merged #276).
- [x] `rara-persistence` crate: `atomic_file`, `redaction`, `thread_data`, `thread_metadata`, `thread_rollout_log`, `thread_turn_log`, `file_lock` (merged #279, #282, #283, #338+).
- [x] Code health review (see `docs/features/code-health-review-2025.md`).

### P0 — Oversized module split (blocking)

- [ ] Split `src/tui/state/mod.rs` (2000+ lines): extract types, presets, persistence, provider-status into submodules; shrink mod.rs to ≤300 lines facade.
- [ ] Remove dead code from production source files: 71 `#[allow(dead_code)]` sites. Priority: `src/runtime_control.rs` (scaffolding), `src/hook_registry.rs`, `src/acp_consumer.rs`, `src/mcp_status.rs`.

### P1 — Agent and compaction modules

- [ ] Split `src/agent.rs` (1287 lines): extract tool-execution, plan-handling, history-management into `agent/` submodules.
- [ ] Split `src/agent/compact/main.rs` (~1250 lines): split by compaction phase (microcompact, full-compact, strategy).
- [ ] Narrow `Agent` struct field visibility from `pub` to `pub(crate)` or private; add accessor methods where needed.

### P2 — Render and context modules

- [ ] Split `src/tui/render.rs` (990 lines): move remaining top-level functions into existing render submodules.
- [ ] Split `src/context/assembler.rs` (916 lines): extract budget calculation and message assembly.
