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

- Replacing `history.json` in the first implementation slice.
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
  transcript.jsonl          # typed main-session transcript
  events.jsonl              # non-turn runtime events
  000000.json               # TUI turn artifact snapshots
  subagents/
    agent-<agent_id>.jsonl  # typed sidechain transcript
```

The first implementation keeps `history.json` as the resume source and writes
`transcript.jsonl` as a typed compatibility mirror. Foreground sub-agent tools
also write parent-scoped sidechain transcripts after each completed invocation.
They also append a parent-session `SpawnAgent` rollout event that records the
generated `agent_id`, child `session_id`, optional display name, status, and summary
without inlining the child transcript. This is intentionally additive: existing
session restore behavior stays unchanged while tests start locking down the
model-visible transcript boundary.

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

## Validation Matrix

| Case | Expected behavior |
| ---- | ----------------- |
| Save ordinary session history | `history.json` and `transcript.jsonl` both exist. |
| Load transcript with malformed line | Valid lines load; parse error count increments. |
| Project model-visible messages | Only non-sidechain `Message` entries are returned. |
| Write sub-agent sidechain | File is under `subagents/`; entries carry `is_sidechain = true`. |
| Record sub-agent spawn edge | Parent rollout events include one `spawn_agent` edge summary with child identity. |
| Legacy history backfill | `history.json` and `transcript.jsonl` are both backfilled. |
| Future resume migration | Resume can switch from `history.json` to transcript projection without reading TUI artifacts. |

## Open Risks

- The first implementation rewrites the transcript mirror from `history.json`.
  The canonical target is append-only, but the compatibility bridge must stay
  consistent with the existing snapshot source until resume migrates.
- Existing foreground sub-agent tools write sidechain transcripts only when
  invoked with parent session context. Direct test calls without parent context
  still return structured results without writing detached sidechain files.
- Sidechain persistence failures are reported through `persistence_error`; they
  do not abort an otherwise completed foreground sub-agent call.
- `StateDb` still stores parent/child relationships only indirectly through
  rollout events. A queryable spawn-edge table is needed before cross-session
  sub-agent resume is complete.
- Context compaction must preserve transcript boundaries and avoid injecting
  summaries before stable prompt-prefix sources.
- Background sub-agent execution, resume, and stop semantics remain future work.

## Source Journals

- 2026-05-04-session-transcript-foundation.md
- 2026-05-05-subagent-sidechain-transcripts.md
