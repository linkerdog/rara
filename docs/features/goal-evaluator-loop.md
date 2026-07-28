# `/goal` Evaluator Loop

## Status
- [x] Evaluator placeholder wired into agent turn cycle
- [x] Real LLM evaluator call through the backend classifier boundary
- [x] `GoalCreated` / `GoalCompleted` hook lifecycle phases

## Design

### Flow
```
User: /goal "run cargo test && all 14 pass"

Loop:
  1. Agent runs normally (tools + reasoning)
  2. Turn completes
  3. Evaluator asks the classifier boundary for `yes` or `no: <reason>`
  4. Agent sees evaluator context on next turn → continues working
  5. Agent eventually calls `update_goal(status=Complete)` or user stops
```

### Implementation
- `RalphGoal.condition: Option<String>` set from objective at create_goal time
- Evaluator call runs in `tasks/goal_evaluator.rs` after each successful
  non-plan goal turn and after budget accounting
- Uses `app.push_system()` to push evaluator context as System message
- Agent loop continues via existing `start_query_task()` with next_goal_prompt

### Evaluator
The evaluator calls `LlmBackend::classify()` with:
```
The goal is: {condition}
Based on the work done in the last turn, is the goal satisfied?
Answer ONLY "yes" or "no: <one-sentence reason>".
```
- "yes" → mark Complete, push notice
- "no: reason" → push reason as System message, continue loop

The current implementation reuses the active backend classifier path. A
dedicated small-model route can be added behind the same boundary once auxiliary
model routing is explicit in config.

### What's NOT included
- Dedicated small-model provider selection for the evaluator
- Automatic execution semantics for `GoalCreated` / `GoalCompleted` command
  hooks
- Prompt injection (evaluator context is a transcript message, not a prompt fragment)

### Hook Lifecycle Phases

`GoalCreated` and `GoalCompleted` are stable hook lifecycle phase names in the
shared hook lifecycle enum, app-server control-plane declarations, and plugin
`hooks.json` registration.

The phase names are declaration-compatible before command execution is enabled
for goal lifecycle hooks. Execution needs a separate input-payload and
permission contract.

## Validation

- Evaluator parser accepts exact `yes` and `no: reason` classifier output.
- Goal turn completion marks the goal complete and stops the continuation loop
  when the evaluator returns `yes`.
- Goal turn completion injects `no: reason` as system context and continues the
  loop when the evaluator returns `no`.
- Plugin `hooks.json` registers `GoalCreated` and `GoalCompleted` lifecycle
  hooks.
- App-server hook control declarations serialize and deserialize goal lifecycle
  phases.

## Source Journals

- `docs/journal/2026-07-29-goal-evaluator-loop.md`
