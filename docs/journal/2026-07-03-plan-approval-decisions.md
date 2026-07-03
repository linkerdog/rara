# 2026-07-03 Plan Approval Decisions

## Summary

- Replaced boolean plan approval control input with an explicit
  `PlanApprovalDecision` enum.
- Added `approve`, `continue_planning`, and `reject` decision handling across
  the control plane, agent continuation, and TUI pending interaction flow.
- Updated the plan approval dock copy to use short action labels:
  `approve`, `keep planning`, and `reject`.

## Background

The previous `AnswerPlanApproval { approved: bool }` shape collapsed two
different non-approval outcomes into one path. That made it hard to distinguish
"keep planning" from "cancel this implementation request" and kept the TUI copy
too close to a yes/no prompt.

Claude Code separates final plan approval from clarification questions, and
Codex-style review decisions use explicit semantic outcomes. RARA now follows
that direction with a narrow plan-approval decision enum.

## Scope

- `approve` resumes execution with the approved plan.
- `continue_planning` resumes the agent loop in planning mode.
- `reject` clears the pending approval, records the decision, and does not
  start a new model turn.
- The control request carries optional feedback for protocol adapters. The TUI
  does not collect free-form feedback yet.

## Validation

- `cargo test tui::tests::pending_plan_approval_number_shortcuts_work_in_local_and_ssh`
- `cargo test tui::tests::plan_approval_reject_clears_pending_without_starting_task`
- `cargo check --locked --workspace --all-targets`
- `cargo clippy --locked --workspace --all-targets -- -D warnings`

## Follow-Ups

- Persist planning lifecycle decisions in the structured rollout log.
- Restore pending plan approval after restart.
- Add editable approval UI if continue/reject feedback should be collected from
  the local TUI.
