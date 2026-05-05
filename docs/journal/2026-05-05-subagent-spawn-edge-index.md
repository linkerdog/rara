# Sub-Agent Spawn Edge Index

## Checkpoint

RARA now mirrors parent-scoped `SpawnAgent` rollout events into a queryable
`StateDb` table. The append-only rollout log remains the durable event source;
the SQLite table is an index for listing, resume, and later stop operations.

## Runtime Boundary

- `spawn_agent_edges` stores parent session id, event id, agent id, optional
  display name, child session id, status, summary, and recorded timestamp.
- `ThreadStore::load_thread` synchronizes the index from structured rollout
  events while materializing a thread snapshot, but skips the database write
  when the indexed edge set already matches the rollout log.
- The parent model context still only receives the compacted tool result and
  does not inline sidechain transcript content.

## Validation

- `indexes_spawn_agent_edges_for_listing_queries`
- `load_thread_aggregates_history_state_and_rollout_items`

## Follow-up

- Background sub-agent resume/stop should query this index instead of scanning
  parent rollout files directly.
