# Goal Evaluator Loop

## What Changed

- Added a goal evaluator module that calls `LlmBackend::classify()` and parses
  exact `yes` / `no: reason` answers.
- Replaced the fixed goal-loop placeholder with evaluator-driven completion and
  continuation behavior.
- Added `GoalCreated` and `GoalCompleted` lifecycle phase names to shared hook
  lifecycle declarations and plugin hook registration.

## Why

The goal loop should not continue blindly after every turn. A classifier-style
evaluator provides a narrow completion check while preserving the existing
agent loop and `update_goal` tool contract.

## Trade-Offs

- The evaluator currently uses the active backend classifier path instead of a
  dedicated small-model route. This keeps the implementation provider-neutral
  and avoids adding config surface before auxiliary model routing is explicit.
- Goal lifecycle hook phases are declaration-compatible, but command execution
  for those phases remains deferred until the input payload and permission
  semantics are specified.

## Remaining Work

- Add explicit auxiliary-model routing if the evaluator should use a different
  provider/model from the main agent.
- Define and implement command execution semantics for goal lifecycle hooks.
