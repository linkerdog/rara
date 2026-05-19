# `/goal` Evaluator Loop

## Status
- [x] Evaluator placeholder wired into agent turn cycle
- [ ] Real LLM evaluator call (small model, yes/no output)
- [ ] `GoalCreated` / `GoalCompleted` hook phases

## Design

### Flow
```
User: /goal "run cargo test && all 14 pass"

Loop:
  1. Agent runs normally (tools + reasoning)
  2. Turn completes
  3. Evaluator pushes "no: goal not yet complete — ..." as System message
  4. Agent sees evaluator context on next turn → continues working
  5. Agent eventually calls `update_goal(status=Complete)` or user stops
```

### Implementation
- `RalphGoal.condition: Option<String>` set from objective at create_goal time
- Evaluator placeholder injected in `tasks.rs` Ralph loop after budget check
- Uses `app.push_system()` to push evaluator context as System message
- Agent loop continues via existing `start_query_task()` with next_goal_prompt

### Evaluator (future)
The real evaluator will call a lightweight LLM with:
```
The goal is: {condition}
Based on the work done in the last turn, is the goal satisfied?
Answer ONLY "yes" or "no: <one-sentence reason>".
```
- "yes" → mark Complete, push notice
- "no: reason" → push reason as System message, continue loop

### What's NOT included
- Real LLM evaluation (placeholder always returns "not yet")
- Goal lifecycle hook phases (GoalCreated / GoalCompleted)
- Prompt injection (evaluator context is a transcript message, not a prompt fragment)
