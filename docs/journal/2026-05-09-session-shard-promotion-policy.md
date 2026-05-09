# Session Shard Promotion Policy Gate

## Checkpoint

Periodic session-shard promotion now has an explicit opt-in policy gate. The
existing manual promotion API remains unchanged, while scheduler-style callers
can evaluate a `SessionShardPromotionPolicy` before writing durable memory.

## Runtime Contract

- Default policy is disabled, so background promotion cannot write memory by
  accident.
- Enabled policy requires enough checkpoints and a non-zero maximum checkpoint
  window.
- Promotion attempts return a structured `SessionShardPromotionOutcome` with
  trigger, checkpoint counts, decision, skip reason, and promoted count.
- Runtime memory events can carry that outcome without depending on logs.

## Follow-Up

The policy gate does not install a timer. A future scheduler can call the gated
API from a runtime task once configuration, status display, and operator controls
are ready.
