# Transcript Canonical Restore and Compaction Event Metadata

## Summary

This checkpoint moves session history restore to the typed transcript boundary
and enriches persisted compaction events with lifecycle metadata.

## Changes

- `rollouts/<session_id>/transcript.jsonl` is now the first source used by
  `SessionManager::load_thread_history_migration`.
- `history.json` remains as a compatibility snapshot and fallback source.
- Session checkpoints append new transcript messages when the existing typed
  transcript is a prefix of the latest model history, and rewrite only when
  history was replaced.
- `ThreadTranscriptRecorder` centralizes append, rewrite, flush, and shutdown
  behavior for the typed transcript file.
- Raw session-context checkpoints now write to per-session `context.jsonl`
  shards under `rollouts/<session_id>/`; session recall and
  `MemoryRetrievalOrchestrator` read those shards instead of the former global
  LanceDB `conversations` table.
- Damaged transcripts with parse errors fall back to `history.json` when a
  snapshot exists, then rewrite a clean transcript projection.
- Legacy `.rara/sessions/<session_id>.json` restore still backfills both
  `history.json` and `transcript.jsonl`.
- Compaction rollout events now carry optional `replaced_start`,
  `replaced_end`, and `metadata_owner` fields while preserving compatibility
  with older event JSON.
- Runtime partial compaction now accepts `from` / `up_to` ranges that align with
  API-round boundaries, preserves the unchanged prefix/suffix, and persists the
  selected replaced range in the compaction event.
- Compaction summary generation retries structured context-window failures by
  dropping the oldest API-round group from the summary input; unrelated provider
  errors still follow the normal automatic/manual failure behavior.
- Post-compact carry-over sources now have stable descriptors and surface through
  `/context`; `/resume` includes compact boundary/range/token metadata in the
  thread picker row.
- Runtime and fork checkpoints now enter through `ThreadRecorder`, write the
  canonical transcript first, then write the compatibility history snapshot
  separately, including compaction-triggered checkpoints.
- History checkpoints now update canonical `transcript.jsonl` before the
  compatibility `history.json` snapshot. If the transcript write fails, the
  snapshot is not advanced ahead of the canonical model-history source.
- Hook and MCP retain hints from compacted history now become stable
  `history.compaction.hooks` and `history.compaction.mcp` carry-over sources.
- `ThreadRecorder` now exposes append/flush/shutdown operations for structured
  rollout items, and compaction event writes use that append-only recorder path.
- Runtime manual/automatic compaction now persists lifecycle events through
  `ThreadRecorder` instead of calling the session compatibility façade directly.
- Restore now treats empty transcripts and shorter transcript prefixes as
  recoverable when a `history.json` snapshot exists, repairs the transcript from
  that snapshot, and keeps snapshot restores usable even if transcript repair is
  blocked.
- Session-context shards now append each checkpoint as one locked JSONL line and
  skip malformed lines during retrieval so one truncated line does not break
  recall.
- Transcript, context-shard, and rollout-log directory syncs are best-effort on
  supported platforms, so unsupported directory handles do not make checkpoints
  fail.
- Agent checkpoint and compaction-event writes reuse the agent's `StateDb`
  handle, and session-context save/search work is moved off the async executor
  with `spawn_blocking`.
- Compatibility `history.json` snapshots now fsync the temporary file before
  atomic replacement and best-effort sync the parent directory after replacement.
- Transcript checkpointing now keeps an mtime/size-validated in-process cache
  for exact path lookups, avoiding a full transcript reparse on every prefix
  append without exposing `HashMap` iteration order to prompt or context
  rendering.
- `ThreadStore` now stores explicit rollout and legacy-session roots and
  materializes transcript, snapshot, legacy-history, and compaction migrations
  directly from those roots. `SessionManager` remains as a compatibility
  constructor/wrapper, not the materialization dependency.
- `ThreadRecorder` now normalizes runtime rollout snapshots and appends the
  canonical `runtime_state` event directly to `events.jsonl`, leaving `StateDb`
  side tables as compatibility/index surfaces.
- Committed TUI turns now append to per-session `turns.jsonl` through
  `ThreadRecorder`; `ThreadStore` prefers that log over `StateDb` turn rows and
  only uses the SQLite rows as a legacy fallback.
- Runtime rollout snapshots are materialized as snapshots: `ThreadStore` uses
  the latest `runtime_state` event for plan/interactions instead of expanding
  stale snapshots into duplicate rollout items.
- Thread runtime metadata now writes to per-session `thread.json` before
  updating the `StateDb` index row. `ThreadStore` prefers that structured
  metadata file and can materialize a thread without a `StateDb` session row.

## Remaining Work

- Continue shrinking `StateDb` side-table fallback usage for plan/interactions
  now that history, metadata, turns, runtime rollout state, and compaction events
  have structured per-session sources.
- Move stable compaction retain hints into the real hook/MCP execution paths
  once those runtime extension points produce context source descriptors
  directly.
