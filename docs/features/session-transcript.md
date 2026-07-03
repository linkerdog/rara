# Session Transcript

## Problem

RARA currently persists model history, TUI transcript artifacts, runtime rollout
events, plan state, and todo state through separate files. The separation is
useful, but the primary model history is still a full JSON snapshot
(`rollouts/<session_id>/history.json`) while runtime events are JSONL and turn
artifacts are per-turn JSON arrays.

That mixed shape makes the resume boundary harder to reason about:

- model-visible messages can be confused with TUI-only transcript artifacts;
- sub-agent output needs first-class sidechain identity;
- fork/resume work has no stable typed event stream to filter by semantic role;
- future ACP/Wire subscribers cannot follow one ordered transcript contract.

## Scope

This spec defines a typed session transcript format for RARA sessions and
sub-agent sidechains.

It covers:

- the local file layout;
- typed JSONL entries;
- model-visible vs sidechain filtering;
- parent/child agent relationships;
- the migration path from `history.json`.

## Non-Goals

- Replacing all snapshot writes with append-only transcript checkpoints in the
  first implementation slice.
- Replacing `StateDb`; SQLite remains the listing/index surface.
- Storing full sub-agent output inline in the parent session transcript.
- Persisting TUI rendering cells as model-visible messages.
- Designing the full remote/appserver thread store.

## Architecture

RARA should converge on a Claude-style transcript layout with Codex-style typed
events:

```text
rollouts/<session_id>/
  history.json              # compatibility snapshot, not the long-term source
  thread.json               # canonical thread metadata
  transcript.jsonl          # typed main-session transcript
  turns.jsonl               # committed TUI turn summaries and entries
  context.jsonl             # per-session context-retrieval shard
  events.jsonl              # non-turn runtime events
  000000.json               # TUI turn artifact snapshots
  subagents/
    agent-<agent_id>.jsonl  # typed sidechain transcript
```

The current implementation reads `transcript.jsonl` first when restoring
model-visible history. `history.json` remains as a compatibility snapshot and
fallback source. Runtime checkpoints enter through `ThreadRecorder`, write that
typed transcript through `ThreadTranscriptRecorder`, then write per-session
thread metadata and the compatibility snapshot separately. `ThreadStore`
materialization reads the rollout root directly for metadata, transcript,
snapshot, legacy-history, and compaction migration sources; `SessionManager`
remains a compatibility entry point for older callers. The recorder appends new
message entries when the existing
transcript is a prefix of the new history, exposes flush/shutdown boundaries,
and falls back to an atomic transcript rewrite when history was replaced by
repair or compaction. If a transcript has parse errors and a snapshot exists,
restore falls back to the snapshot and rewrites a clean transcript projection.
Empty transcripts and shorter transcript prefixes also fall back to the
snapshot when one exists, which preserves crash recovery after partial
checkpoint writes. Foreground sub-agent tools also write parent-scoped
sidechain transcripts after each completed invocation. They also append a
parent-session `SpawnAgent` rollout event that records the generated `agent_id`,
child `session_id`, optional display name, status, and summary without inlining
the child transcript.

Parent-scoped sub-agent calls also register a child `StateDb` session row with
`origin_kind = subagent` and `forked_from_thread_id = <parent_session_id>`.
Plan steps, plan explanation, and pending request-input state are copied into
the child row so `ThreadStore` does not need to fabricate metadata from
history-only files.

If sidechain or spawn-edge persistence fails after the child agent has already
completed, the tool call should still return the child result and include a
structured `persistence_error` field. This keeps foreground delegation useful
while making the missing sidechain explicit to the parent agent and TUI.

The long-term target is:

- `transcript.jsonl` becomes the canonical model-history stream;
- `history.json` becomes an optional acceleration snapshot or disappears;
- `StateDb` indexes sessions, turns, plan state, and parent/child edges;
- `thread.json` is the canonical thread metadata source for materialization;
- sidechain transcripts remain separate files and are never replayed into the
  parent model context unless an explicit fork/context operation selects a
  filtered subset.

## Contracts

### Entry Types

All transcript files are JSONL: one typed JSON object per line.

Current schema:

```rust
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionTranscriptEntry {
    SessionMeta {
        schema_version: u32,
        session_id: String,
        parent_session_id: Option<String>,
        agent_id: Option<String>,
        is_sidechain: bool,
    },
    Message {
        message_id: String,
        parent_message_id: Option<String>,
        session_id: String,
        agent_id: Option<String>,
        is_sidechain: bool,
        role: String,
        content: serde_json::Value,
    },
    SpawnAgent {
        event_id: String,
        session_id: String,
        child_session_id: Option<String>,
        agent_id: Option<String>,
        name: Option<String>,
        status: String,
        summary: Option<String>,
    },
}
```

### Model Visibility

Only `Message` entries with `is_sidechain == false` may be projected into the
parent model context.

Sidechain entries are durable and inspectable, but they are not parent context.
The parent session may store a `SpawnAgent` summary event, but it must not
inline the child transcript.

### Parent Links

`parent_message_id` is a local transcript chain link. It gives the future
resume path a stable ordered chain without requiring the TUI turn artifacts to
be replayed as text.

### Sub-Agent Sidechains

Sub-agent transcripts use:

```text
rollouts/<parent_session_id>/subagents/agent-<agent_id>.jsonl
```

Each sidechain entry carries:

- `parent_session_id`;
- `agent_id`;
- `is_sidechain = true`;
- its own `session_id` when the sub-agent runtime has one.

### Fork Context

Forking a child agent from parent context should use a filtered transcript
projection:

- keep system/developer/user messages and assistant final answers;
- drop reasoning, raw tool calls, tool results, TUI progress, approval UI, and
  runtime-control metadata;
- preserve prefix stability by avoiding parent runtime-event reinjection before
  stable prompt sources.

This mirrors Codex's fork filtering while keeping Claude-style sidechain files.

### TUI Turn Rendering

Committed TUI turn rendering must preserve the recorded entry order. `You`,
progress entries (`Thinking`, `Exploring`, `Planning`, `Running`), assistant
messages, system notices, interaction completions, tool calls, and terminal
events are all part of one time-ordered turn projection. The renderer may merge
adjacent progress entries with the same role to avoid noisy duplicate headers,
but it must not globally sort entries by role, move approvals ahead of earlier
messages, or replace a mixed tool turn with only the final assistant message.

Live active-turn rendering may still use runtime event state for streaming
thinking and progress tails, but those live events have the same ordering
contract: event order is the user-visible ordering boundary, with only adjacent
same-role progress compaction allowed.

Fallback rendering for plain tool and tool-progress messages keeps the newest
tail lines when the message exceeds the main-view line budget. Recent command
or tool output is usually the actionable state; full-output fidelity belongs to
terminal cells and transcript-detail surfaces.

## Validation Matrix

| Case | Expected behavior |
| ---- | ----------------- |
| Save ordinary session history | `history.json` and `transcript.jsonl` both exist. |
| Append ordinary checkpoint | New model-visible messages append to `transcript.jsonl` when the existing transcript is a prefix. |
| Append committed turn | New TUI turn entries append to `turns.jsonl`; `StateDb` turn rows remain compatibility/index data. |
| Load transcript with malformed line | Valid lines load; parse error count increments. |
| Project model-visible messages | Only non-sidechain `Message` entries are returned. |
| Write sub-agent sidechain | File is under `subagents/`; entries carry `is_sidechain = true`. |
| Record sub-agent spawn edge | Parent rollout events include one `spawn_agent` edge summary with child identity. |
| Run background sub-agent | Tool result returns `agent_id`, `session_id`, and `status = running` without inlining the child transcript. |
| Resume background sub-agent | `subagent_resume` returns the live status or, after runtime restart, reconnects to the current thread's persisted completed sidechain result without loading the sidechain into parent context. |
| Stop background sub-agent | `subagent_stop` marks an in-process running sub-agent as `cancelled` and requests model cancellation. |
| Legacy history backfill | `history.json` and `transcript.jsonl` are both backfilled. |
| Transcript-first restore | `transcript.jsonl` wins over a stale `history.json` snapshot. |
| Damaged transcript fallback | Transcript parse errors fall back to `history.json` when available and rewrite a clean transcript. |
| Empty or short transcript fallback | Empty transcripts or shorter transcript prefixes fall back to `history.json` and repair the transcript. |
| Turn materialization | `ThreadStore` prefers `turns.jsonl` over stale `StateDb` turn rows. |
| Render committed mixed turn | `You`, thinking/exploring/running, tool calls, approvals, terminal output, and agent messages render in recorded order. |
| Render long fallback tool message | The main view shows a hidden-earlier-lines marker plus the newest tail lines. |
| Render live progress turn | Streaming thinking and progress events render in event order while adjacent same-role progress may compact. |

## Open Risks

- `SessionManager::save_session` remains as a compatibility wrapper. Runtime
  checkpoints use `ThreadRecorder`, but older direct callers can still enter
  through `SessionManager`.
- Existing foreground sub-agent tools write sidechain transcripts only when
  invoked with parent session context. Direct test calls without parent context
  still return structured results without writing detached sidechain files.
- Sidechain persistence failures are reported through `persistence_error`; they
  do not abort an otherwise completed foreground sub-agent call.
- `StateDb` indexes parent/child spawn edges from rollout events. Background
  sub-agent control exposes `subagent_list`, `subagent_resume`, and
  `subagent_stop`; restart/reconnect can reattach to completed persisted
  sidechain results, but continuing a still-running task after process exit
  still needs a durable task registry above the sidechain transcript contract.
- Context compaction must preserve transcript boundaries and avoid injecting
  summaries before stable prompt-prefix sources.
- Background sub-agent execution is still local to the active RARA process; a
  process exit stops in-flight child execution even though completed results can
  be reconnected later.

## Source Journals

- 2026-05-04-session-transcript-foundation.md
- 2026-05-05-subagent-sidechain-transcripts.md
- 2026-05-05-subagent-spawn-edge-index.md
- 2026-05-05-subagent-background-control.md
- 2026-07-03-subagent-reconnect.md
