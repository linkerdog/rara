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
- Durable lifecycle, usage, creation time, and clear across thread restoration.

## Non-Goals

- Multi-goal scheduling.
- A separate multi-goal management screen.
- Auxiliary-model planning or compression for goals.
- A runtime classifier or durable counter that second-guesses model goal
  completion or blocked-state decisions.

## Architecture

`RalphGoal` is runtime-owned session state shared through `GoalHandle`. Commands,
model-facing tools, and turn accounting use one serialized mutation boundary.
When thread persistence is enabled, that boundary writes SQLite before
publishing a new in-memory snapshot. A failed write surfaces an error and keeps
the previous snapshot; no continuation may launch from a failed mutation.
The TUI holds only a presentation snapshot and delegates writes to the runtime.

The existing `goals` row stores every lifecycle status, budget, usage counter,
and the original creation timestamp. Clearing deletes that row. Replacing a
completed goal starts a new creation time; updating an existing goal retains
its creation time. Restoration uses checked deserialization: unknown statuses,
negative or oversized counters, empty objectives, zero budgets, and invalid
timestamps fail rather than become an active goal. A missing row clears any
prior thread's presentation snapshot. Thread restoration stages required
thread/todo/runtime reads before publishing the agent, TUI snapshot, or new
goal binding; a failed required read retains the previous thread and its
binding. Initial bootstrap and thread resume both warn and disable goal
persistence when the optional goal binding is unavailable, without aborting
the whole runtime or silently accepting memory-only goals. Resume still
publishes the requested thread and clears the prior goal snapshot; it does
not rewrite the unreadable row. A later valid thread restore can re-enable
durable goal mutations.
Runtime rebuilding preserves the current goal's thread binding and state.
Persistence-disabled embedded profiles remain explicitly in-memory.

Successful non-plan queries capture their goal membership and input-token
baseline at turn start. They persist usage and increment the turn counter even
when the goal becomes complete, blocked, or paused during that query. The
resulting status controls continuation, not accounting. Pending questions and
shell approvals stop further continuation without losing successful-turn usage.
Queued user work takes precedence over another substantive goal continuation;
the budget-limit wrap-up still takes precedence over new goal work.
Clearing or replacing the goal, or switching threads, prevents the old query from charging the new
goal. Queries that start without an active goal do not charge a goal created
later in that query.
Errored/cancelled queries, plan-only queries, and the budget-limited wrap-up
query are not charged under the current successful-active-query policy. The
counters are not an all-provider-spend ledger.

The lifecycle is:

- `Pursuing`: runtime may auto-continue after tool-using turns.
- `Paused`: user/TUI paused the goal; model tools cannot set this.
- `Blocked`: model reported a repeated, genuine blocker; user/TUI can resume
  it as a fresh blocked-state audit.
- `Complete`: model marked the goal complete through `update_goal`.
- `BudgetLimited`: runtime marked the goal over budget and asks for a wrap-up.

The TUI owns local lifecycle controls:

- `/goal <objective>` creates a goal when none exists or replaces a completed
  goal; replacing an unfinished goal requires an explicit local confirmation.
  It immediately starts its first
  continuation when the session is idle and the runtime agent is ready.
- `/goal --tokens <N> <objective>` creates a budgeted goal.
- `/goal pause`, `/goal resume`, and `/goal clear` mutate local lifecycle state.
  Resume accepts paused, blocked, or idle pursuing goals (including interrupted
  work and a failed enqueue); resuming a blocked goal restarts its
  audit and immediately starts a continuation turn when the session is idle.
  Resume refuses to change state while another task is running or the runtime
  has no agent to execute the continuation.
- A failed `/goal` mutation displays a diagnostic, retains the committed
  snapshot, and keeps the command loop alive without launching continuation.
- Resuming a paused/blocked goal whose stored usage meets its budget persists
  `BudgetLimited` and starts only the existing wrap-up prompt, not another
  substantive continuation. A failed status write starts neither path.
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

### Explicit Thread Resume

An explicit startup resume, continue, thread ID, or picker selection arms one
continuation for a restored pursuing goal. Runtime bootstrap alone does not.
Admission waits for a ready agent, an idle session, execute mode, no pending
interaction, no queued user input, and no active overlay. Admission checks the
current goal again; a new turn, thread restore, clear, replacement, lifecycle
change, or user stop invalidates an older request. A backend rebuild preserves
this identity. Waiting for readiness must not drop or duplicate the request.
Plan mode keeps an automatic request pending until execute mode is eligible;
it does not silently discard the request. Rejected or failed goal admission
retains runtime ownership of the ready agent and starts no task.

A user cancellation or interruption durably defers automatic continuation
without changing the goal's lifecycle. A new explicitly started turn clears
this deferral. Goal writes retain it until that boundary; clear or replacement
resets it. Persistence errors remain visible and must not permit automatic work.
The additive SQLite flag defaults to false for existing rows, whose historical
interrupts were never recorded; persisted paused/blocked/complete/budget-limited
states still never start automatically. Invalid flag values reject restoration.

Restored active goals already at their token limit start only the budget wrap-up
after persisting the budget-limited state. Automatic work retains the current
permission mode and normal approval path. Its notice includes `Resuming goal:`,
the objective, `/goal pause`, and the current permission mode. Interrupted goals
stay idle with an explicit resume hint. A paused goal offers `Resume paused
goal?`, with `Resume goal` and `Leave paused`; dismissal leaves it paused.

### Local Goal Interaction

Bare `/goal` opens a summary with status, objective, elapsed wall time since
creation, completed turns, tokens, budget, and state-appropriate commands.
`/goal edit` pre-fills the objective for editing, preserving budget and usage;
editing alone does not resume inactive work. Replacing an unfinished goal asks
for confirmation and validates the original goal identity before committing.
These controls do not loosen the model-facing `create_goal` restriction.
`/goal pause` may pause future continuations while the current turn finishes.
The compact indicator names every lifecycle state and displays elapsed time
for unbounded goals or used/budget for bounded goals. Timer-driven updates are
coalesced at the displayed time granularity, only when the visible elapsed second changes.

### Snapshot Numeric Safety

The runtime `GoalStore` checks token budgets, used tokens, and completed turns
against the in-memory `u32` range. An invalid or out-of-range numeric field
rejects that goal with a visible notice while keeping the thread resumable and
the stored data untouched. The TUI disables goal persistence after a failed
restore; it must not wrap counts, turn an invalid budget into an unlimited goal,
or keep another thread's in-memory goal. A healthy subsequent restore re-enables
durable writes. No separate presentation-local goal decoder owns this contract.

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

- lifecycle badge: `Active`, `Paused`, `Blocked`, `Complete`, or `BudgetLimited`;
- elapsed seconds for unbounded goals, or turn count for budgeted goals;
- token usage with explicit `tokens` units;
- remaining budget when present.

Detailed goal state belongs in `/goal`, not the bottom pane.

## Validation Matrix

- Tool schema exposes `complete` and `blocked` as `update_goal` statuses.
- `create_goal` rejects empty objectives, zero budgets, oversized budgets, and
  unfinished goals, while allowing a completed goal to be replaced.
- `update_goal` rejects every status other than `complete` and `blocked`.
- `/goal --tokens 98.5K <objective>` parses human-readable budgets.
- `/goal` asks before replacing an unfinished goal, replaces a completed one,
  and starts a newly created or resumed goal with a continuation turn. Confirming
  stale replacement/edit state cannot mutate another goal; dismissing a choice
  preserves the old goal. Editing retains usage, budget, status, and deferral.
- A resumed goal does not add its internal continuation prompt to the user
  transcript.
- A blocked goal produces its final user-facing blocker report in the same turn
  and does not continue automatically afterward.
- Continuation prompts include untrusted objective boundaries and budget fields.
- A pursuing goal continues without an out-of-band completion classifier or a
  classifier-injected system reason.
- Budget-limit prompts ask for wrap-up without new work.
- Explicit startup/latest/picker restoration queues exactly one continuation;
  fresh bootstrap does not. Not-ready, busy, Plan, overlay, pending-interaction,
  and queued-user-work gates defer admission without losing it. Recheck the
  goal revision at consumption and reject stale or duplicate commands.
- Interrupted work remains deferred across mutation, rebuild, and restart.
  A new turn clears deferral durably; a failed clear leaves it set. A late stop
  preserves an already completed turn's result and usage, while deferring its
  next continuation. Legacy SQLite migration preserves existing lifecycle data;
  invalid deferral values reject restoration.
- Paused resume choices, full summaries, replacement confirmation, Unicode
  editing, and state-specific commands render through the production harness.
  Elapsed display invalidation occurs only when the displayed second changes.
- Bottom-pane rendering keeps the goal label compact and uses `tokens` units.
- Production thread restoration rejects overflowing counters/budgets without
  rewriting stored rows, then admits exact `u32` limits after repair. Rejection
  disables goal persistence until a healthy restore, retaining main runtime ownership.
- Fresh-app round trips preserve all five statuses, counters, budgets, and the
  original creation time through both command and tool mutations.
- Clear remains absent after restoration, including a switch from another
  thread with a goal. Replacement uses the new goal's creation time.
- Write failures leave the prior snapshot unchanged and stop continuation;
  corrupt stored status, objective, budget, or numeric values cannot restore
  as `Pursuing`. Command failures do not exit the TUI event loop.
  A goal-accounting failure still publishes the returned agent's completed
  runtime snapshot and session metadata before stopping; the failed goal
  usage write does not roll back the successful query itself.
- Corrupt todo/runtime JSON retains the prior thread and goal binding.
  Corrupt goals preserve bootstrap and requested/latest-thread restoration
  with a warning and fail-closed goal tools, without altering either thread's
  durable goal row; a subsequent valid restore re-enables persistence.
  Explicit and generated session IDs
  address the same durable binding as the assembled agent.
- Complete and blocked snapshots include the final successful goal turn after
  fresh-app restoration, without launching another continuation. Non-goal and
  plan queries do not charge a newly created or inactive goal.

## Operational Notes

### Recovering Unavailable Goal Persistence

An unreadable goal row disables goal mutations for the resumed session while
leaving its thread history available. The restore notice/log records the
diagnostic. `/goal clear` cannot delete the unreadable row or repair the durable
binding, because the active snapshot was cleared during failed restoration. Creating a
replacement goal also fails with the persistence-unavailable error. There is
no in-app repair command.

For operator-managed recovery:

1. Record the affected thread ID and diagnostic. Stop all processes using the
   state database and create a verified SQLite backup before editing it; do
   not edit a live database or reset the entire state database.
2. Inspect only the `goals` row whose `session_id` equals that exact thread ID.
   Repair invalid fields from known-correct data using the validation rules
   above. If the goal cannot be recovered, removing that single row requires
   explicit operator approval to discard its objective, status, and counters.
   Keep the backup and leave unrelated goal, thread, and runtime rows intact.
3. Restart and resume the same thread. A successfully validated row restores
   its goal; an intentionally absent row restores no goal and permits a new
   one. Successful restoration re-enables durable mutations. If validation
   still fails, retain the diagnostic and keep writes disabled.

Resuming a different healthy thread can re-enable persistence for that thread,
but does not repair the original row. Recovery must not invent a `Pursuing`
status, zero usage, or a fresh creation time to hide corrupted data.

## Open Risks

- Older binaries did not record interruption deferral. Migration preserves
  their lifecycle but cannot recover an unrecorded user stop.
- If saving an interruption fails, the TUI warns that the goal may resume after
  restart. The pending in-process admission is invalidated even on write failure.
- An abrupt process crash does not persist a user-stop deferral. A pursuing
  goal can retry the same work on each explicit thread resume, including work
  that repeatedly panics or exhausts memory. There is no crash-loop counter;
  successful-turn accounting does not charge incomplete crashed turns.
- The three-turn blocked audit is intentionally prompt/tool-contract enforced,
  like Codex, rather than a second runtime state machine. A malicious or weak
  model can still misuse the tool, so provider behavior should be observed.
- Budget accounting is based on available input-token deltas. Providers that do
  not report usage precisely may undercount.

## Source Journals

- `docs/journal/2026-05-08-codex-129-goals.md`
- `docs/journal/2026-09-16-codex-v0154-goals.md`
- `docs/journal/2026-09-16-goal-resume-permission-tui.md`
- [Unicode boundary checkpoint](../journal/2026-10-03-unicode-boundaries.md)
- `docs/journal/2026-10-02-thread-goal-persistence.md`
- [Goal resume and local controls](../journal/2026-10-03-goal-resume.md).
- [TUI review follow-ups](../journal/2026-10-04-tui-review-followups.md).
