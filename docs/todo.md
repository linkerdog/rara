# TODO

Active backlog only. Keep this file small and current.

## Suggested Rollout Order

1. Runtime control plane and ACP/Wire-ready context contracts
2. Runtime bootstrap and source-object unification
3. Configuration and provider-surface cleanup
4. Workspace / skill observability and cache correctness
5. Memory / retrieval / thread persistence
6. TUI transcript parity and command-surface polish
7. Terminal-Bench evaluation readiness

## Runtime Control Plane / ACP / Wire

- [x] Define adapter-neutral runtime control request/event types for ACP, Wire, TUI, CLI, and future appserver entrypoints (see `docs/features/runtime-control-plane.md`).
- [x] Add Claude-style `todo_write` runtime state with session persistence, TUI update cards, and structured Wire/ACP-ready events.
- [x] Add source-aware MCP config registry for user `config.toml` and project `.mcp.json` with duplicate-name conflict failure.
- [ ] Route ACP prompt/cancel/session handling through the normal RARA runtime path instead of the current stub.
- [x] Add protocol subscriber plumbing on top of the structured `AgentEvent` runtime-control bridge.
- [x] Add MCP connection manager status model (`configured`, `connecting`, `connected`, `refreshing`, `reconnecting`, `failed`, `disabled`) from `McpRegistry`.
- [x] Add `/mcp` status surface grouped by scope and source path.
- [x] Publish `/mcp` status snapshots as structured runtime events for future ACP/Wire/appserver subscribers.
- [ ] Add dynamic MCP tool/resource/prompt refresh through structured runtime events.
- [ ] Add bounded MCP auto-reconnect with manual reconnect command.
- [ ] Add MCP resource references as source objects visible in `/context`.
- [ ] Add MCP Tool Search so large MCP tool sets are discovered on demand instead of injected into every prompt.
- [ ] Support protocol-registered prompt sources with provenance, scope, budget hints, and `/context` visibility.
- [ ] Support protocol-registered skill sources through the same `SkillRegistry` precedence and override reporting as local skills.
- [ ] Add protocol-safe memory mutation/query scaffolding that creates memory records and selection views without bypassing `MemorySelection`.
- [ ] Add hook declaration scaffolding for protocol and repo extensions; keep execution disabled until permission and sandbox policy are explicit.
- [ ] Ensure every new skill, memory, prompt, hook, planning, approval, and output feature is control-plane-ready rather than TUI-only.

## Configuration / Provider Surface

- [ ] Complete `reasoning_summary` rollout across backend requests, switching flows, and status surfaces; retire remaining `thinking`-only behavior outside migration fallback.
- [ ] Surface provider-scoped reasoning configuration in `/status` and provider/model switching flows.
- [ ] Study Gemini/Codex-style multi-model routing for top-tier + flash/fast model pairing.
- [ ] Deepen provider-surface continuity after hot-swap: auth-mode/endpoint alignment, provenance reporting.
- [ ] Align Codex endpoint selection with auth mode (ChatGPT/Codex login vs API key).
- [ ] Split Codex-specific persisted auth/config to `~/.codex`, keep RARA config under `~/.rara`.

## Workspace / Skills / Prompt Sources

- [ ] Tests for workspace prompt-source discovery and cache invalidation (cwd changes, git branches, nested workspaces).
- [ ] Define `WorkspaceMemory` cache invalidation rules for prompt files and environment info.
- [ ] Unify `discover_prompt_sources()` and TUI `/status` source reporting.
- [ ] New prompt inputs through structured source objects, `MemorySelection`, lifecycle events, and runtime-control provenance — not ad hoc text.
- [ ] Project-scoped extension surface for `.claude/agents/`, `.claude/hooks/`, `.agents/skills/` with precedence rules.
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
- [ ] Upgrade thread distillation from summary capture to LLM-assisted 2-8 record extraction with deduplication.
- [x] Move raw session checkpoints into per-session append shards instead of the global LanceDB `conversations` table.
- [x] Promote `rollouts/<session_id>/transcript.jsonl` from compatibility mirror to canonical model-history source.
- [x] Wire foreground sub-agent tools to write parent-scoped sidechain transcripts under `rollouts/<parent_session_id>/subagents/`.
- [x] Add append-only parent/child spawn-edge rollout metadata for foreground sub-agent calls.
- [x] Index parent/child spawn-edge metadata in `StateDb` for resume/listing queries.
- [x] Add in-process background sub-agent resume/stop over the sidechain transcript contract.
- [ ] Add periodic promotion from session shards into global `MemoryRecord`s.
- [x] Durable in-turn checkpoints: persist after each message/tool-result batch, atomic writes, crash-tolerant `SessionManager`.
- [ ] Define cross-process background sub-agent restart/reattach semantics.
- [x] Compaction as first-class lifecycle event: persist summaries, token counters, metadata ownership.
- [x] Add prompt-too-long retry for compaction by dropping oldest API-round groups.
- [x] Add partial compact support around a selected message boundary (`from` / `up_to`).
- [x] Add post-compact source descriptors and surface them in `/context` and `/resume`.
- [x] Add generic memory/hook/skill/MCP carry-over consumer shape that validates stable compaction source descriptors.
- [x] Add concrete retrieved-memory carry-over producer.
- [x] Add concrete invoked-skill carry-over producer.
- [x] Add concrete hook/MCP carry-over producers.
- [ ] Surface main model vs auxiliary model routing in `/status` and `/context`.
- [x] `ThreadStore` / `ThreadRecorder`: from façade over `SessionManager`+`StateDb` to true structured thread store.
- [ ] Thread-scoped and workspace-scoped `MemoryRecord` storage with promotion rules.
- [ ] Retrieval orchestration layer from `docs/features/context-architecture.md`.
- [x] Initialize LanceDB and wire FTS/vector/hybrid search paths behind the existing memory index façade.
- [ ] Make memory mutation/query control-plane-ready so ACP/Wire can inspect and add memory without directly editing prompt text.

## TUI / Transcript

- [ ] Decouple overlays from transcript layout (pure top layer, no viewport perturbation).
- [ ] Post-exit resume hint (e.g. `rara resume --last`).
- [ ] Claude-style repo context hints beneath input area (GitHub PR link).
- [ ] Codex/Claude-style transcript role cards (`You` / `Agent` / `System`).
- [ ] Stabilize active response blocks while streaming, avoid generic transcript fallback.
- [ ] Rework built-in command TUI (`/help`, `/model`, `/status`, command palette, overlays) to match Codex/Claude.
- [ ] Refine `/status`: provider/model state, reasoning, sandbox/network, context injection, tool availability.
- [ ] Tool-action summaries more source-aware and file-aware.
- [ ] Live `bash` transcript: lifecycle framing, streamed stdout/stderr, long-output folding.
- [ ] High-fidelity render pass for `write/update`, inline diffs, approval cards, message-card hierarchy.
- [ ] Expand TUI snapshot coverage.
- [ ] Keep transcript and pending-interaction state backed by structured events that ACP/Wire output subscribers can reuse.
- [x] Add first-class todo sections to `/context` and `/status` from `TodoContextView`.

## Security / Reliability / Performance

- [x] Structured command model (`program`, `args`, `cwd`, `allow_net`) in `src/tools/bash.rs` and `crates/sandbox`.
- [ ] Classifier and routing model from `docs/features/classifier-and-routing.md`.
- [ ] Auditable permission + sandbox-bypass rules (Codex/Claude-inspired);
      segment-level bash prefix reuse is implemented, but typed approval-policy
      modes and full rule provenance remain open.
- [ ] Structured auto-permission classifier with compact transcript projection.
- [ ] Background-task state classifier: `working` / `blocked` / `done` / `failed`.
- [ ] `secrecy::SecretString` end-to-end for API keys, audit error paths.
- [ ] Replace `.expect(...)` with structured `anyhow::Context` errors.
- [ ] Review path/command validation in `bash`, file tools, sandbox.
- [ ] Rework token accounting in `src/agent.rs` (avoid re-encoding full history).
- [ ] Replace fixed 100ms TUI event polling loop.

## Evaluation / Benchmarks

- [ ] Terminal-Bench readiness: add a headless adapter, preserve structured trajectories, and start with a small smoke run that records RARA revision, dataset version, provider/model, sandbox mode, and failure taxonomy.

## Code Organization / Docs

- [ ] Continue splitting remaining oversized runtime/TUI files, especially `src/tui/render/cells.rs`, `src/tui/render.rs`, `src/tui/command.rs`, and `src/agent/planning.rs`.
