# TUI Subagent Status Projection

## Summary

RARA now projects session-scoped child-agent lifecycle and progress into the
TUI while the root turn is still running. The wide sidebar and `/status`
overview show the real agents registered in `AgentTreeControl`, including
their kind, lifecycle status, provider/model route, tool count, token count,
and latest bounded activity.

## Background

The runtime already tracked child agents, but the presentation snapshot did
not expose that state. The sidebar's former `Sub-agents` section read pending
approval and request-input interactions instead, so it could display unrelated
prompts as running agents and could not display actual children.

The implementation follows two upstream patterns reviewed before coding:

- Codex projects typed child-thread status into a bounded agent status feed and
  excludes raw reasoning from activity previews.
- Claude Code derives coordinator status from structured agent task state and
  accumulated progress rather than parsing tool-result text.

## Implementation

- Child `AgentEvent` values update the existing `SubagentProgress` record.
  Tool uses and sanitized status messages become bounded activity; assistant
  and reasoning deltas remain excluded.
- `AgentTreeControl` exposes an immutable, presentation-safe activity
  projection containing the root session's full descendant tree.
- `RuntimeClient` retains the session tree handle and root identity even while
  the root `Agent` is owned by an asynchronous query task.
- The existing TUI tick refreshes the activity projection and redraws only
  when the value changes.
- Sidebar and `/status` rendering use semantic lifecycle markers and colors.

## Trade-offs

- The TUI polls the in-memory projection on its existing 166 ms tick instead
  of adding a second event protocol. This keeps lifecycle ownership in the
  runtime and bounds display latency without coupling child events to TUI
  rendering types.
- Live input-token usage remains provider-dependent. The final completion
  snapshot replaces provisional counters with the runtime's authoritative
  totals.
- Completed records remain visible according to the existing agent-tree
  retention policy; the sidebar limits its rendering to five records.

## Validation

- Verify typed child events update tool, token, and bounded activity state.
- Verify projections contain only children of the active root session.
- Verify pending approvals are not rendered as subagents.
- Verify the sidebar and `/status` render running agent identity and progress.
- Run Rust formatting and warning checks for the touched workspace.

## Follow-ups

None for this checkpoint.
