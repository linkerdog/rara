# Thread Goals

## Problem

RARA has a persistent `/goal` loop that lets a user ask the agent to keep
working across turns. The older contract allowed the model to mark a goal as
`achieved` or `unmet`, but that made completion semantics loose and gave the
model control over states that should belong to the runtime or TUI.

Codex 0.129 narrows this surface:

- the model can create a goal only when explicitly asked;
- the model can only mark an existing goal as `complete`;
- pause, resume, clear, and budget-limited states are controlled outside the
  model-facing update tool;
- budget and elapsed-time usage are visible in the tool result and TUI.

RARA should mirror that shape while keeping its local TUI command surface.

## Scope

- Model-facing tools: `get_goal`, `create_goal`, `update_goal`.
- Local TUI command: `/goal`.
- Automatic goal continuation prompts.
- Goal budget accounting and budget-limit wrap-up.
- Compact bottom-pane status for active goals.

## Non-Goals

- Multi-goal scheduling.
- Goal persistence across process restarts.
- A full Codex-style goal confirmation menu.
- Auxiliary-model planning or compression for goals.

## Architecture

`RalphGoal` remains the in-memory runtime state shared by the TUI and
model-facing goal tools through `GoalHandle`.

The lifecycle is:

- `Pursuing`: runtime may auto-continue after tool-using turns.
- `Paused`: user/TUI paused the goal; model tools cannot set this.
- `Complete`: model marked the goal complete through `update_goal`.
- `BudgetLimited`: runtime marked the goal over budget and asks for a wrap-up.

The TUI owns local lifecycle controls:

- `/goal <objective>` creates a goal when none exists.
- `/goal --tokens <N> <objective>` creates a budgeted goal.
- `/goal pause`, `/goal resume`, and `/goal clear` mutate local lifecycle state.
- `/goal` shows the current objective, lifecycle state, elapsed seconds, turns,
  tokens used, budget, and remaining tokens.

The model-facing tool contract is intentionally narrower:

- `create_goal` fails if any goal exists.
- `update_goal` accepts only `status: "complete"`.
- `get_goal` returns a structured object plus `remainingTokens`.
- completing a budgeted goal returns `completionBudgetReport` so the model can
  report final token usage without guessing.

## Contracts

### Tool Response Shape

`get_goal`, `create_goal`, and `update_goal` return:

```json
{
  "goal": {
    "objective": "...",
    "status": "active",
    "token_budget": 50000,
    "tokens_used": 0,
    "turns_completed": 0,
    "time_used_seconds": 0
  },
  "remainingTokens": 50000,
  "completionBudgetReport": null
}
```

When no goal exists, all three top-level fields are present and nullable.

### Continuation Prompt

Automatic continuation wraps the objective in `<untrusted_objective>` so the
stored objective cannot override higher-priority instructions. The prompt also
includes:

- elapsed time;
- tokens used;
- token budget;
- tokens remaining;
- a completion audit instruction before calling `update_goal`.

### Budget Limit Prompt

When the runtime marks a goal as `BudgetLimited`, it starts one final wrap-up
turn instead of silently stopping. That turn must summarize completed work,
remaining blockers, and the next safe step. It must not start new substantive
work, and it must not call `update_goal` unless the objective is actually
complete.

### TUI

The bottom pane should show only compact state:

- lifecycle badge: `active`, `paused`, `done`, or `budget`;
- turn count;
- token usage with explicit `tokens` units;
- remaining budget when present.

Detailed goal state belongs in `/goal`, not the bottom pane.

## Validation Matrix

- Tool schema exposes only `complete` as an `update_goal` status.
- `create_goal` rejects empty objectives, zero budgets, oversized budgets, and
  duplicate active goals.
- `update_goal` rejects all statuses except `complete`.
- `/goal --tokens 98.5K <objective>` parses human-readable budgets.
- `/goal` refuses to replace an existing goal without an explicit clear.
- Continuation prompts include untrusted objective boundaries and budget fields.
- Budget-limit prompts ask for wrap-up without new work.
- Bottom-pane rendering keeps the goal label compact and uses `tokens` units.

## Open Risks

- Goal state is still in-memory. Restart persistence should be considered only
  after the session-state boundary is stable.
- RARA does not yet have Codex's full confirmation menu for replacing a goal.
  The current safer behavior is to require `/goal clear` first.
- Budget accounting is based on available input-token deltas. Providers that do
  not report usage precisely may undercount.

## Source Journals

- `docs/journal/2026-05-08-codex-129-goals.md`
