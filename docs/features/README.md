# Feature Docs Standard

This directory stores stable, domain-oriented technical specifications.
Chronological implementation records belong in `docs/journal/`.

## Required Structure

Each active feature spec should include:

- `Problem`
- `Scope`
- `Non-Goals`
- `Architecture`
- `Contracts`
- `Validation Matrix`
- `Open Risks`
- `Source Journals`

## File Policy

- `docs/features/`: stable theme/domain docs only
- `docs/journal/`: date-prefixed implementation records
- When a feature evolves, update the canonical feature doc and add or append a journal note.

## Current Persistence Specs

- `session-transcript.md`: typed session and sub-agent transcript storage.

## Control-Plane Readiness

Features that affect skills, memory, prompt sources, hooks, planning, approvals,
tool output, `/context`, or `/status` must describe how the behavior can be
driven through the runtime control plane. Local TUI behavior should be one
adapter over the same structured request/event contract that ACP, Wire, and
future appserver integrations can use.

## Security And Approval Specs

- `shell-approval-policy.md`: bash read-only classification and reusable prefix
  approval boundaries.
- `sandbox-execution.md`: capability-based shell isolation, structured process
  termination, and evidence-backed sandbox failure reporting.

## Runtime Extension Specs

- `mcp-runtime.md`: source-aware MCP configuration, registry, status, refresh,
  reconnect, resource, and Tool Search contracts.
- `support-acp-integration.md`: ACP client integration guidance boundary,
  semantic input intents, output subscriptions, source registration, provenance,
  and trust rules.
- `bedrock-backend.md`: Bedrock SDK backend crate boundary and RARA adapter
  contract.
- `file-search.md`: shared gitignore-aware file discovery and fuzzy path
  ranking crate for tools, TUI pickers, and context routing.
- `retrieval-orchestration.md`: unified candidate-provider, ranking, dedupe,
  budget, and `/context` contract for memory/context retrieval.
- `observability.md`: bounded process-local runtime metrics and `/status`
  context/metrics contracts.
- `local-embedding-runtimes.md`: local Python model server, macOS MLX/Qwen3
  backend, portable FastEmbed/ONNX backend, and server safety contract.
- `workspace-memory-cache.md`: prompt-source and workspace-memory cache
  invalidation, ordering, and shared observability contract.
- `thread-goals.md`: persistent `/goal` runtime, tool, continuation, budget,
  and compact TUI contracts aligned with Codex 0.130.
- `subagent-context-optimization.md`: bounded parent/child subagent context
  inheritance, child budget, result summary, and restart/reconnect contracts.
- `shared-task-lists.md`: workspace-local shared task store and
  Claude-compatible read tools for future subagent/team coordination.
- `hooks-plugin-lifecycle.md`: hook/plugin lifecycle phases, MemoryQuery
  dispatch, and hook output context injection boundaries.
- `tui-theme-tokens.md`: configurable semantic TUI theme tokens, renderer
  integration, and embedded syntax theme selection.
- `wasm-core.md`: pure Rust browser/worker core boundary for deterministic
  patch preview and future protocol/reducer logic.

## App Server Architecture

- `app-server-architecture.md`: agent output as typed objects over a lightweight
  event bus; TUI, ACP, Wire as peer consumers. Internal = objects, external =
  protocol.

## Crate Split

- `crate-split.md`: dependency-ordered split of the monolithic binary crate
  into `rara-app`, `rara-agent`, `rara-tui`, `rara-memory`, `rara-tools`,
  `rara-control-plane`, `rara-oauth`, and `rara-state`. Each PR green on
  `cargo test`.

## Release Specs

- `release-distribution.md`: tag-driven binary release, GitHub Release assets,
  Homebrew adapter, and npm adapter contracts.
