# Subagent Context Optimization

## Problem

RARA can delegate work through `spawn_agent`, `explore_agent`, `plan_agent`,
and `team_create`, but subagent context must stay bounded as teams start using
delegation for larger tasks.  A child agent should not receive the full parent
transcript by default, and the parent should not ingest a full child transcript
when the child finishes.

The target behavior is a parent/child context contract: children inherit only
the context needed for their task, then return structured summaries and
artifacts back to the parent.

## Scope

- Define the context inherited by foreground and background subagents.
- Define what child results return to the parent context.
- Define budget accounting for child prompts and parent result injection.
- Define restart/reconnect expectations without requiring implementation in
  this slice.
- Align `spawn_agent`, `explore_agent`, `plan_agent`, and `team_create` around
  one policy model.

## Non-Goals

- Implement durable cross-process background subagent reattachment.
- Change tool permission scoping or Claude-compatible agent definition loading.
- Stream full child transcripts into the parent TUI.
- Replace `ThreadStore` or sidechain transcript persistence.
- Add a new user-visible command surface.

## Architecture

### Context Layers

Subagents receive context in stable-to-volatile order:

1. stable runtime instructions and tool policy;
2. selected project/user instructions that apply to the child task;
3. the delegation contract from the parent;
4. compacted parent summary when available and relevant;
5. task-scoped retrieved files, memories, or diagnostics;
6. a bounded recent parent turn suffix only when needed.

The default must be task-first, not transcript-first.  The delegation
instruction is always present.  Parent history is optional and budgeted.

### Child Budget Policy

Each child prompt has its own budget derived from the active model context
window.  The parent should pass a `SubagentContextPolicy` equivalent with:

- maximum inherited parent tokens;
- maximum retrieved context tokens;
- whether compacted parent summary is allowed;
- whether recent parent turns are allowed;
- expected result summary budget.

`team_create` applies the same policy per child.  The aggregate parent result
is separately budgeted so one verbose child cannot starve sibling summaries.

### Parent Return Path

The parent receives structured child output:

- child id and kind;
- status;
- short result summary;
- changed files or produced artifacts when available;
- validation commands and status;
- follow-up suggestions;
- pointer to the child sidechain transcript.

The parent must not inject the full child transcript into normal context.  A
human or future resume tool may inspect the sidechain transcript explicitly.

### Restart And Reconnect Boundary

Foreground subagents are complete once the tool returns.  Background subagents
need durable metadata before cross-process restart can be reliable:

- parent session id;
- child session id;
- agent kind/name;
- task contract;
- last known status;
- sidechain transcript pointer;
- resumable command or reconnect capability, if the backend supports it.

Until that durable registry exists, background reattachment after process exit
is unsupported.  The runtime should report that limitation explicitly instead
of pretending the in-memory registry is durable.

## Contracts

- `spawn_agent`, `explore_agent`, and `plan_agent` use the same inherited
  context contract, with different tool permissions and task framing.
- `team_create` must validate all child task contracts before starting any
  child work.
- Parent context receives summaries and artifact pointers, not raw child
  transcripts.
- Child sidechain transcripts remain inspectable through explicit transcript or
  resume surfaces.
- Context budget reporting must distinguish parent context, inherited child
  context, and returned child summaries.
- Background subagent list/resume/stop surfaces must report whether the record
  is process-local or durably reconnectable.

## Validation Matrix

| Area | Validation |
| --- | --- |
| Inheritance | Unit test that child context excludes unrelated parent turns when a compacted summary is available. |
| Delegation contract | Unit test that every subagent prompt includes the task instruction and selected project instructions. |
| Team budget | Unit test that `team_create` applies per-child limits and bounded aggregate summaries. |
| Return path | Tool-result test that parent context includes child summary, artifact pointers, and sidechain id, but not full transcript. |
| Background status | Runtime test that process-local background records report non-durable reconnect status. |
| TUI/API surfaces | Snapshot or control-plane test showing child summaries render as delegation objects. |

## Operational Notes

- A child that needs more context should request it through an explicit tool or
  follow-up question rather than inheriting the entire parent transcript.
- If a child discovers durable project knowledge, it should promote that fact
  through memory/project-memory mechanisms instead of relying on parent
  transcript injection.
- The policy should remain provider-neutral; provider-specific model windows
  only influence derived token limits.

## Open Risks

- Too little inherited context may cause child agents to repeat discovery work.
- Too much inherited context can erase the token savings of delegation.
- Background subagent reconnect can become misleading unless process-local and
  durable states are clearly separated.
- Summary-only return paths require enough structure to preserve validation
  evidence and changed-file provenance.

## Source Journals

- `context-architecture.md` — parent/child thread model and staged rollout.
- `subagent-and-aux-compression.md` — progress tracking and compression
  background.
- `subagent-claude-compat.md` — Claude-compatible agent definition direction.
