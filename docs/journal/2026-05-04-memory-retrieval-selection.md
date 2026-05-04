# Memory Retrieval Selection Checkpoint

## Summary

M1 and M2 are now wired as one runtime slice:

- `MemoryRetrievalOrchestrator` searches the session-context LanceDB table and
  the durable `MemoryStore`.
- Search hits become direct `RetrievedMemoryCandidate` values.
- `MemorySelection` ranks those candidates against the discretionary retrieval
  budget and reports selected, available, and dropped outcomes.
- Selected retrieval candidates are sent to the model as a per-turn internal
  context block prepended to the current user request content, not as part of
  the stable system prompt and not as persisted conversation history.

## Reference Pattern

The injection shape follows the reference-agent boundary:

- Codex keeps base instructions separate from contextual user fragments such as
  `AGENTS.md` and environment updates.
- Peer agent runtimes render memory as contextual attachments and keep
  prompt-cache-sensitive text stable when possible.

RARA should keep the same invariant for future memory work: retrieval may change
per turn, but stable prompt sources should keep their ordering and bytes.

## Validation

Focused coverage was added for:

- direct retrieved candidates selected when budget allows;
- retrieved candidates dropped when the budget is exhausted;
- `/context` assembly showing selected retrieval under `active_memory_inputs`;
- agent model input receiving internal memory context without persisting it into
  `history` or creating consecutive user-role messages.
- tool-result follow-up turns reusing the same retrieval candidates without
  prepending memory context to tool-result messages.

## Remaining Work

- Move raw session checkpoints into per-session append shards.
- Add periodic promotion from session shards into global `MemoryRecord`s.
- Add retention and pinning policy before any automatic cleanup path exists.
