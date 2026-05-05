# Sub-Agent Sidechain Transcripts

## Checkpoint

Foreground sub-agent tools now receive parent session context through
`ToolCallContext` and write completed child histories to parent-scoped
sidechain transcript files:

```text
rollouts/<parent_session_id>/subagents/agent-<agent_id>.jsonl
```

Each sub-agent result includes the generated `agent_id` and child `session_id`.
The parent receives only the structured tool result summary; the child history
stays in the sidechain file and is not projected into parent model-visible
messages.

Foreground calls also append a parent-session `SpawnAgent` rollout event. The
event stores the parent/child edge metadata needed by later resume and stop
work: generated `agent_id`, child `session_id`, optional display name, status, and
summary. The edge is intentionally stored as rollout metadata instead of parent
model-visible text, preserving the context prefix boundary.

If persistence fails after the child agent completes, the foreground tool result
now includes `persistence_error` instead of failing the whole tool call. Explicit
sub-agent names that cannot produce a stable ASCII id label are rejected before
any child agent starts.

## Runtime Boundary

- `spawn_agent`, `explore_agent`, `plan_agent`, and `team_create` generate a
  per-invocation `agent_id`.
- Calls made from the main agent carry the parent `session_id` through
  `ToolCallContext`.
- Calls made directly without parent context return structured results but do
  not write detached sidechain files.
- Sidechain transcripts use `TranscriptScope::sidechain`, so every entry is
  marked `is_sidechain = true`.
- Parent rollout events store the compact spawn edge. The parent transcript
  remains free of child transcript content.
- Persistence failure is result metadata, not a fatal tool error, once the child
  agent has completed.

## Validation

- `team_create_writes_parent_scoped_sidechain_transcripts`
- `subagent_without_parent_context_does_not_write_sidechain`
- `team_create_writes_parent_scoped_sidechain_transcripts` also asserts the
  `SpawnAgent` rollout event.
- `team_create_rejects_unstable_explicit_name_before_running_subagents`
- `subagent_returns_result_when_sidechain_persistence_fails`
- Existing `model_visible_messages` coverage confirms sidechain messages are not
  parent-visible context.

## Follow-Up

- Index parent/child spawn-edge metadata in `StateDb` for query and resume
  surfaces.
- Add background sub-agent resume and stop semantics on top of the same
  sidechain transcript contract.
