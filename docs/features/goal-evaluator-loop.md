# `/goal` Evaluator Loop (Claude Code-style)

## Motivation

Claude Code's `/goal` is a runtime loop: after every turn, a lightweight
evaluator assesses whether the goal condition is met.  If not, the loop
continues automatically with the evaluator's feedback as context.

This differs from Codex's hidden-prompt-injection approach.  The agent
doesn't know it's in a goal — it only sees the evaluator's "not yet"
message at the start of each continuation turn.

## Design

### Flow

```
User: /goal "run cargo test && all 14 pass"

Loop:
  1. Agent runs normally (tools + reasoning)
  2. Turn completes
  3. Evaluator (small model) checks condition:
     "Is 'run cargo test && all 14 pass' satisfied?"
     → "no: 3 of 14 tests still fail"  → loop back to 1
     → "yes: all 14 tests pass"        → set Complete, return to user
```

### Evaluator

The evaluator is a lightweight LLM call with a fixed prompt:

```
The goal is: {objective}

Based on the work done in the last turn, is the goal satisfied?
Answer ONLY "yes" or "no: <one-sentence reason>".
```

- Uses `LlmBackend::complete()` with a small, fast model
- Max ~100 tokens output
- Result parsed: starts with "yes" → Complete; starts with "no:" → continue

### State Machine

`RalphGoal` gains a `condition: Option<String>` field.

| GoalStatus | Set when |
|---|---|
| `Pursuing` | Goal created, evaluator running |
| `Complete` | Evaluator returns "yes" |

`Paused` and `BudgetLimited` are removed (Claude Code has no budget tracking).

### Agent Loop Integration

In the main turn loop (`tasks.rs`), after a turn completes:

```
if let Some(goal) = app.goal_handle.read().unwrap().as_ref()
    && goal.status == Pursuing
{
    let evaluator_result = run_evaluator(&llm, &goal.condition).await?;
    match evaluator_result {
        EvaluatorResult::Satisfied => {
            app.goal_handle.write().unwrap().as_mut().unwrap().status = Complete;
            app.push_notice("Goal completed");
            break; // return control to user
        }
        EvaluatorResult::NotSatisfied(reason) => {
            // Continue loop — evaluator's reason becomes context
            app.push_entry("System", reason);
            continue;
        }
    }
}
```

### Prompt

The evaluator's "not yet" reason is pushed as a System message:
```
System: no: 3 of 14 tests still fail — check src/auth.rs:42
```

This becomes the agent's context for the next turn. The agent sees it
and continues working, same as if a user said "3 tests still fail".

### `create_goal` Tool

```
create_goal(objective: "run cargo test && all 14 pass")
```

Creates `RalphGoal { status: Pursuing, condition: Some("run cargo test && all 14 pass") }`.

### `update_goal` Tool

```
update_goal(status: Complete)
```

Explicitly marks the goal complete (user can also force-complete).

## Integration Points

- `agent.rs` — evaluator loop called at turn end
- `runtime_context.rs` — creates evaluator LLM instance (separate from main LLM)
- `tui/state/types.rs` — `RalphGoal.condition: Option<String>`
- `tools/goal.rs` — `create_goal` sets condition

## What's NOT included

- Prompt injection (no `<goal>` fragments)
- Token budget tracking
- `Paused` / `BudgetLimited` states
- `with_goal()` builder on ContextAssembler

## Verification

- Manual: `/goal "echo hello"` → agent runs echo → evaluator says yes → complete
- Manual: `/goal "run cargo test && all 14 pass"` with broken tests → loops until fixed
