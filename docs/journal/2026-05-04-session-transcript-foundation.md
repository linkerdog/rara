# Session Transcript Foundation

## Context

RARA had three relevant persistence surfaces:

- `rollouts/<session_id>/history.json` for model history;
- `rollouts/<session_id>/events.jsonl` for structured non-turn runtime events;
- `rollouts/<session_id>/<ordinal>.json` for TUI turn artifacts.

Codex and Claude Code both use typed transcript/rollout streams as the durable
conversation surface. Claude Code keeps sub-agent sidechains in separate
agent-scoped JSONL files, while Codex keeps spawned agents as separate threads
with typed spawn edges and filtered fork history.

## Change

Added a typed transcript foundation:

- `src/session_transcript.rs` defines `SessionTranscriptEntry`.
- Main session transcripts write to `rollouts/<session_id>/transcript.jsonl`.
- Sub-agent sidechain paths are defined as
  `rollouts/<parent_session_id>/subagents/agent-<agent_id>.jsonl`.
- `SessionManager::save_session` now writes the existing `history.json`
  snapshot and a typed transcript mirror.
- Legacy history backfill also writes the typed transcript mirror.
- Transcript loading is tolerant of malformed lines and reports a parse-error
  count.
- `model_visible_messages` projects only non-sidechain message entries.

## Boundary

This is an additive compatibility bridge. `history.json` remains the active
resume source for this slice. The new transcript file gives later work a tested
typed target without changing session restore semantics in the same PR.

## Follow-Up

- Promote `transcript.jsonl` from mirror to canonical model-history source.
- Wire existing `spawn_agent`, `explore_agent`, and `plan_agent` tools to write
  parent-scoped sidechain transcripts.
- Add durable spawn-edge metadata in `StateDb`.
- Add fork-context filtering that drops tool/runtime/reasoning entries before
  handing parent context to a child agent.
