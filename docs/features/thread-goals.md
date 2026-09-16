# Thread Goals

## Problem

RARA has a persistent `/goal` loop that lets a user ask the agent to keep
working across turns. RARA previously added an out-of-band classifier that
could complete a goal independently of the agent's `update_goal` call. That
made a second model decision authoritative and diverged from Codex's goal
contract.

Codex 0.154 defines this surface:

- the model can create a goal only when explicitly asked;
- the model can mark an existing goal as `complete` or `blocked`;
- a completed goal can be replaced, while every unfinished goal rejects a new
  `create_goal` call;
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
- Durable synchronization of every lifecycle mutation across process restarts.
- A full Codex-style goal confirmation menu.
- Auxiliary-model planning or compression for goals.
- A runtime classifier or durable counter that second-guesses model goal
  completion or blocked-state decisions.

## Architecture

`RalphGoal` remains in-memory session state shared by the TUI and model-facing
goal tools through `GoalHandle`. The local command may save a goal snapshot,
and session restoration recognizes every lifecycle status present in that
snapshot; durable synchronization of every lifecycle mutation is out of scope.

The lifecycle is:

- `Pursuing`: runtime may auto-continue after tool-using turns.
- `Paused`: user/TUI paused the goal; model tools cannot set this.
- `Blocked`: model reported a repeated, genuine blocker; user/TUI can resume
  it as a fresh blocked-state audit.
- `Complete`: model marked the goal complete through `update_goal`.
- `BudgetLimited`: runtime marked the goal over budget and asks for a wrap-up.

The TUI owns local lifecycle controls:

- `/goal <objective>` creates a goal when none exists or replaces a completed
  goal; it rejects every unfinished goal and immediately starts its first
  continuation when the session is idle and the runtime agent is ready.
- `/goal --tokens <N> <objective>` creates a budgeted goal.
- `/goal pause`, `/goal resume`, and `/goal clear` mutate local lifecycle state.
  Resume accepts paused and blocked goals; resuming a blocked goal restarts its
  audit and immediately starts a continuation turn when the session is idle.
  Resume refuses to change state while another task is running or the runtime
  has no agent to execute the continuation.
- `/goal` shows the current objective, lifecycle state, elapsed seconds, turns,
  tokens used, budget, and remaining tokens.

The model-facing tool contract is intentionally narrower:

- `create_goal` fails only if an unfinished goal exists and replaces a
  completed goal.
- `update_goal` accepts `status: "complete"` or `status: "blocked"`.
- `blocked` is valid only when the same blocker recurs for at least three
  consecutive goal turns and no meaningful progress is possible without user
  input or an external-state change. This is enforced by the tool instruction,
  matching Codex; the runtime does not keep a competing hidden audit counter.
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
stored objective cannot override higher-priority instructions. It does not run
a separate completion classifier: the main agent must audit actual state and
call `update_goal` itself. The prompt also includes:

- elapsed time;
- tokens used;
- token budget;
- tokens remaining;
- completion and blocked-state audit instructions before calling `update_goal`.

When the model marks a goal blocked, it must finish that same turn with the
blocking condition, attempted work, required user input or external change,
and the safe condition for `/goal resume`. The blocked state then stops further
automatic continuation.

Continuation prompts are runtime control messages, not user-authored
transcript entries. The TUI shows goal progress through the compact status
surface rather than exposing the internal continuation prompt as a new `You`
message.

### Budget Limit Prompt

When the runtime marks a goal as `BudgetLimited`, it starts one final wrap-up
turn instead of silently stopping. That turn must summarize completed work,
remaining blockers, and the next safe step. It must not start new substantive
work, and it must not call `update_goal` unless the objective is actually
complete.

### TUI

The bottom pane should show only compact state:

- lifecycle badge: `active`, `paused`, `blocked`, `done`, or `budget`;
- turn count;
- token usage with explicit `tokens` units;
- remaining budget when present.

Detailed goal state belongs in `/goal`, not the bottom pane.

## Validation Matrix

- Tool schema exposes `complete` and `blocked` as `update_goal` statuses.
- `create_goal` rejects empty objectives, zero budgets, oversized budgets, and
  unfinished goals, while allowing a completed goal to be replaced.
- `update_goal` rejects every status other than `complete` and `blocked`.
- `/goal --tokens 98.5K <objective>` parses human-readable budgets.
- `/goal` refuses to replace an unfinished goal, replaces a completed one, and
  starts a newly created or resumed blocked goal with a continuation turn.
- A resumed goal does not add its internal continuation prompt to the user
  transcript.
- A blocked goal produces its final user-facing blocker report in the same turn
  and does not continue automatically afterward.
- Continuation prompts include untrusted objective boundaries and budget fields.
- A pursuing goal continues without an out-of-band completion classifier or a
  classifier-injected system reason.
- Budget-limit prompts ask for wrap-up without new work.
- Bottom-pane rendering keeps the goal label compact and uses `tokens` units.

## Open Risks

- RARA does not yet have Codex's full confirmation menu for replacing a goal;
  the local `/goal` command replaces only a completed goal.
- The three-turn blocked audit is intentionally prompt/tool-contract enforced,
  like Codex, rather than a second runtime state machine. A malicious or weak
  model can still misuse the tool, so provider behavior should be observed.
- Budget accounting is based on available input-token deltas. Providers that do
  not report usage precisely may undercount.

## Source Journals

- `docs/journal/2026-05-08-codex-129-goals.md`
- `docs/journal/2026-09-16-codex-v0154-goals.md`
- `docs/journal/2026-09-16-goal-resume-permission-tui.md`
