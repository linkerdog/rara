# Planning Mode

RARA planning mode is a read-only collaboration mode for non-trivial tasks.

## Goals

- let the agent inspect repository context before editing;
- converge toward a concrete implementation plan or a structured clarification;
- preserve planning mode until an explicit runtime action exits it.

## Contract

- planning mode is read-only;
- the agent enters planning mode by calling `enter_plan_mode`; the TUI must not infer planning mode from prompt keywords;
- planning mode persists across turns until the runtime explicitly switches back to execute mode, such as after plan approval;
- decision-complete implementation plans are persisted to `.rara/sessions/<session_id>/plan.md`;
- `exit_plan_mode` submits the persisted proposed plan to the approval flow; it does not directly grant editing permission;
- user imperative wording like "continue" or "implement" does not exit planning mode by itself;
- planning turns may use read-only repository tools, read-only shell commands, and delegated read-only sub-agents;
- planning turns may end with a normal final answer for research, review, or planning-advice tasks;
- when the agent entered planning mode automatically from execute mode without calling `exit_plan_mode`, a decision-complete implementation plan is auto-approved by the runtime and resumes execution without a human approval card;
- manual `/plan` sessions still require explicit human approval before execution resumes from `<proposed_plan>`;
- structured planning artifacts are reserved for explicit runtime actions:
  - `<proposed_plan>` when the implementation plan is ready to persist as the plan artifact;
  - `exit_plan_mode` when the persisted plan is ready for approval;
  - `<request_user_input>` when a key decision still needs user input;
  - `<continue_inspection/>` when more repository inspection is still required.
- a model message with no tool call and no `<continue_inspection/>` is a final answer for the current turn, not an implicit request for runtime continuation;
- planning-mode prose should stay in inspected findings and concise progress updates;
- planning-mode progress updates should stay short and grounded in inspected code instead of narrating each next file-by-file action;
- planning mode must not describe file edits, patches, or implementation steps as if they are already happening.
- plan approval must not be requested in ordinary prose; the model must use `<proposed_plan>` followed by `exit_plan_mode` when the plan is ready, or `<request_user_input>` when a key decision still blocks it.
- `exit_plan_mode` should submit plans through structured tool arguments: `proposed_plan.summary`, `proposed_plan.steps`, and `proposed_plan.validation`.
- For OpenAI native Chat Completions and Responses tool calls, RARA marks strict-compatible tool schemas with `strict: true`, so providers that implement Structured Outputs can enforce the `proposed_plan` argument shape at the API layer.
- `exit_plan_mode` also accepts a complete `<proposed_plan>...</proposed_plan>` block in the same assistant response as a text fallback. A message that opens `<proposed_plan>` without the exact closing `</proposed_plan>` tag is treated as malformed and must not enter approval.
- Markdown headings such as `## Plan`, plain bullets, and prose like "plan ready" are not valid plan submissions. They may be rendered as ordinary assistant text, but they must not trigger plan approval.
- A structured proposed plan should use `summary:`, `steps:`, and `validation:` fields. Runtime treats only entries under `steps:` as executable plan steps; validation bullets are preserved as plan explanation instead of being mixed into the step list.

Plain narration or status updates are valid only when they are the final answer to a research, review, or planning-advice task. They must not be treated as approval requests.

## Sub-Agents

- `explore_agent` is a read-only exploration sidecar for repository inspection;
- `plan_agent` is a read-only planning sidecar for delegated sub-planning;
- sub-agents must not recursively create sub-agents;
- general worker sub-agents do not receive tool access until RARA has an explicit
  nested-agent depth and observability contract;
- planning sub-agents follow the same completion contract:
  - `<proposed_plan>`
  - `<request_user_input>`
  - `<continue_inspection/>`

## Runtime Continuation

When planning mode needs to continue, runtime records a structured continuation message with one of these phases:

- `plan_continuation_required`
- `plan_exit_repair_required`
- `plan_approved`

`plan_continuation_required` means the agent explicitly requested another read-only inspection pass before answering or requesting implementation approval.

`plan_exit_repair_required` means the model called `exit_plan_mode` without a complete same-turn `<proposed_plan>...</proposed_plan>` block. The runtime records a structured repair instruction and gives the model one immediate retry. The retry must either submit a proper `<proposed_plan>` block followed by `exit_plan_mode`, or provide a final planning answer without calling `exit_plan_mode`.

Execute-mode repository inspection follows the same structured continuation boundary: prior inspection evidence is not enough to keep a turn open after a no-tool model response. The model must either call another inspection tool or emit `<continue_inspection/>`.

## TUI Expectations

- planning mode should render as a dedicated planning workflow, not as ordinary execution with extra chatter;
- non-structured planning narration should be folded into planning/exploration sidecars instead of rendering as a separate responding block;
- structured plans should render as an `Updated Plan` checklist object with a separate note/explanation and stable step status markers;
- runtime heartbeat text such as `waiting for model response` or elapsed-time notices should stay in the activity/status surfaces instead of being repeated inside planning, exploration, or updated-plan transcript cells;
- when execution resumes from an approved plan, only the currently active plan step may be auto-completed at successful turn end; pending later steps must not be marked complete optimistically;
- pending plan approval and planning questions should appear as interaction cards;
- status panels, overlays, and transcript cells should reuse the same plan formatting instead of diverging into separate text-only summaries;
- approval or answers should resume the same workflow rather than starting a brand-new free-form chat turn.

## Claude-Style Complete Workflow

The target long-term model is a Claude Code-style planning workflow adapted to RARA's runtime. The current implementation stores a parsed proposed plan as `.rara/sessions/<session_id>/plan.md` and uses `exit_plan_mode` to enter approval. The complete workflow should make that artifact first-class across runtime state, TUI rendering, recovery, and context assembly.

### Plan Artifact

- Each interactive session has a plan artifact at `.rara/sessions/<session_id>/plan.md`.
- The plan artifact is the canonical approval surface for implementation tasks.
- The artifact should contain only the implementation plan, not general chat history or todo progress.
- The artifact should be generated from `<proposed_plan>` until RARA has a dedicated plan-file editing tool.
- The artifact should remain available after approval so execution can refer back to the approved plan.
- A later edit-capable approval UI may update the artifact before approval; the approved artifact version must be what execution receives.

### Todo Relationship

- The plan artifact is not a todo file.
- Plan content answers what will change, why, where, and how it will be verified.
- Todo/checklist state tracks execution progress after approval.
- The todo artifact lives beside the plan as `.rara/sessions/<session_id>/todo.json`; see
  [Todo Runtime](todo-runtime.md) for the detailed contract.
- Todo state must not replace `plan.md` or change plan approval state.

### State Machine

The planning lifecycle should use explicit structured states:

- `execute`: normal coding mode.
- `planning`: read-only exploration and plan drafting.
- `plan_ready`: `exit_plan_mode` has submitted a plan for approval.
- `plan_revising`: the user chose to continue planning after seeing a submitted plan.
- `plan_approved`: the user approved the plan and execution may resume.
- `plan_rejected`: the user rejected the plan without continuing the task.

Allowed transitions:

- `execute -> planning`: user runs `/plan` or the model calls `enter_plan_mode`.
- `planning -> plan_ready`: model emits `<proposed_plan>` and calls `exit_plan_mode`.
- `plan_ready -> plan_approved`: user approves the plan.
- `plan_ready -> plan_revising`: user asks to continue planning or sends the
  plan back with feedback.
- `plan_revising -> plan_ready`: model updates the plan and calls `exit_plan_mode` again.
- `plan_approved -> execute`: runtime injects the approved-plan tool result and resumes the agent loop.
- `plan_ready -> plan_rejected`: user cancels the implementation request.

User imperative text such as "continue", "implement", or "go ahead" must not skip `plan_ready -> plan_approved`; approval must come from the structured TUI interaction.

The control-plane decision is explicit: `AnswerPlanApproval` carries one of
`approve`, `continue_planning`, or `reject`. `approve` resumes execution with
the saved plan, `continue_planning` keeps the agent in planning mode and asks it
to revise the plan, and `reject` clears the pending plan approval without
starting implementation. `continue_planning` and `reject` may carry user
feedback from the approval input; that feedback is forwarded to planning-mode
resume logic instead of being reduced to generic retry or cancellation text.

### Runtime Persistence

Runtime persists the current planning lifecycle in the structured rollout log.
Each runtime-state checkpoint may include one or more plan lifecycle records
with these phases:

- `plan_ready`
- `plan_revising`
- `plan_approved`
- `plan_rejected`

The current checkpoint records the latest phase and decision metadata derived
from the plan approval interaction. Completed decisions should use structured
decision metadata rather than parsing UI copy. Thread forks preserve the
materialized lifecycle records from the source thread. The recovery contract
records enough state to recover a pending approval after restart:

- `session_id`
- `plan_file_path`
- `exit_plan_mode` tool-use id, when pending
- pending approval status
- approved/rejected/continued decision
- approved plan version or content hash
- timestamp of submission and decision

This state should be recorded in the structured rollout log rather than only in
memory. On resume, the latest lifecycle phase is authoritative:

- `plan_ready` restores the approval card and, when present, the
  `exit_plan_mode` tool-use id so approving the restored plan injects the tool
  result once;
- `plan_approved`, `plan_rejected`, and `plan_revising` must not reopen an
  older pending approval card;
- if a plan was rejected or sent back for revision, the model should receive
  structured feedback and remain in planning mode.

### Tool Permissions

Plan mode should be read-only except for plan-artifact updates.

The long-term tool model should replace name-based filtering with structured tool metadata:

- `ReadOnly`: file reads, search, safe status commands.
- `WorkspaceMutation`: edit tools such as `apply_patch`, `replace`, `write_file`.
- `MemoryMutation`: project memory or experience writes.
- `AgentSpawn`: general worker/team launch tools.
- `PlanControl`: `enter_plan_mode`, `exit_plan_mode`.
- `PlanArtifactMutation`: dedicated plan artifact write/update tools.

Plan mode should allow `ReadOnly`, `PlanControl`, and `PlanArtifactMutation`, and block ordinary `WorkspaceMutation`, `MemoryMutation`, and `AgentSpawn`.

### Approval UI

The approval UI should show the persisted plan artifact, not an incidental chat excerpt.

Minimum approval actions:

- approve and start implementation;
- continue planning with feedback;
- reject/cancel implementation.

Future approval actions:

- edit the plan inline before approval;
- open the plan in an external editor;
- compare plan revisions;
- approve with limited command permissions, such as "run tests" or "install dependencies".

When the user edits the plan before approval, execution must receive the edited plan, not the original model output.

### Context Assembly

The current plan artifact should be part of runtime context:

- in planning mode, include the existing plan when re-entering or revising;
- after approval, include a one-time "approved plan" tool result before execution resumes;
- after compaction, preserve the approved plan reference so the model can continue implementation without relying on old chat turns;
- `/context` should display the plan artifact path, status, and whether the plan has been approved.

### Status Surfaces

`/status` should expose planning lifecycle fields:

- current mode;
- plan artifact path;
- plan status;
- pending approval age;
- last approval decision;
- approved plan hash or revision id;
- execution progress against the approved plan, if available.

The current `/status` runtime and overview surfaces expose the lifecycle as a
derived snapshot:

- `planning_status`;
- `plan_path`;
- `planning_pending_age`;
- `planning_last_decision`;
- `approved_plan_revision`;
- `exit_plan_tool`, when a pending `exit_plan_mode` approval is waiting.

`/context` should expose more detail:

- plan source and path;
- prompt/runtime sections that mention plan mode;
- whether current context included the approved plan;
- whether compacted context retained the plan artifact reference.

The current `/context` overlay and text surface include a Planning Lifecycle
section with plan path, approval status, pending age, last decision, approved
revision, and pending `exit_plan_mode` tool-use id when present. Pending plan
approval records include submission timestamps so pending age can render as a
concrete elapsed value. Approved decisions record a stable plan hash so approved
revision can render without depending on UI copy.

### Recovery Scenarios

The complete workflow should handle:

- restart while in `planning`;
- restart while `plan_ready` is waiting for approval;
- restart after approval but before implementation resumes;
- compaction during planning;
- compaction during implementation of an approved plan;
- model switch during planning or after approval;
- user edits plan content before approval;
- stale plan file when a new unrelated task enters plan mode.

Re-entry rules:

- if the new request continues the same task, read and revise the existing plan;
- if the new request is unrelated, replace the existing plan artifact and record a new plan revision;
- do not silently execute an old approved plan for a new request.

### Implementation Plan

Suggested PR sequence:

1. Clarify artifact storage:
   - add explicit `session_artifact_dir` and `session_plan_path` APIs;
   - stop using legacy-history naming for new plan artifacts;
   - add tests for path layout.

2. Persist pending plan approval:
   - add structured rollout events for plan submission and decision;
   - restore pending approval cards from persisted state;
   - ensure approved-plan tool results are injected exactly once.

3. Add tool capability metadata:
   - extend `Tool` with mode capability metadata;
   - replace plan-mode name deny-lists with metadata filtering;
   - keep protocol tool names centralized as constants.

4. Add editable approval UI:
   - render the persisted plan artifact in the approval card;
   - allow continue/reject feedback to be stored with the plan revision;
   - optionally support editing the plan before approval.

5. Improve context and status:
   - include plan status in `/status`;
   - include plan artifact details in `/context`;
   - preserve plan references across compaction and model switching.

6. Retire compatibility auto-approval:
   - require `exit_plan_mode` for implementation-plan approval;
   - keep analysis-only planning answers as normal final answers;
   - update tests and prompts so models do not rely on implicit auto-resume.

### Open Questions

- Should RARA allow the model to directly edit `plan.md`, or should all plan updates continue flowing through `<proposed_plan>` until a dedicated plan-edit tool exists?
- Should plan revisions be separate files, appended structured events, or both?
- Should approval support semantic command permissions like Claude's `allowedPrompts`?
- Should the plan artifact be included in exports or only local session state?
- How should stale plans be garbage-collected?
