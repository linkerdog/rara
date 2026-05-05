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

## Runtime Boundary

- `spawn_agent`, `explore_agent`, `plan_agent`, and `team_create` generate a
  per-invocation `agent_id`.
- Calls made from the main agent carry the parent `session_id` through
  `ToolCallContext`.
- Calls made directly without parent context return structured results but do
  not write detached sidechain files.
- Sidechain transcripts use `TranscriptScope::sidechain`, so every entry is
  marked `is_sidechain = true`.

## Validation

- `team_create_writes_parent_scoped_sidechain_transcripts`
- `subagent_without_parent_context_does_not_write_sidechain`
- Existing `model_visible_messages` coverage confirms sidechain messages are not
  parent-visible context.

## Follow-Up

- Persist durable parent/child spawn-edge metadata in `StateDb`.
- Add background sub-agent resume and stop semantics on top of the same
  sidechain transcript contract.
