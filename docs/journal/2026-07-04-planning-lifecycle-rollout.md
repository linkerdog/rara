# 2026-07-04 Planning Lifecycle Rollout

## Summary

- Added a typed `PlanLifecycle` structured rollout event.
- Persisted plan approval lifecycle phases through runtime-state checkpoints.
- Surfaced plan lifecycle phases in thread snapshot rollout summaries.

## Background

Plan approval already persisted plan snapshots and interaction cards, but the
planning lifecycle itself was implicit in interaction status and summary text.
That made it hard for resume, `/status`, `/context`, and protocol adapters to
distinguish `plan_ready`, `plan_revising`, `plan_approved`, and
`plan_rejected` without string parsing.

## Scope

The implementation keeps the existing runtime-state checkpoint model:
`persist_runtime_state` writes the latest plan state, interactions, and plan
lifecycle into the structured rollout log. The current lifecycle records are
derived from the plan approval interaction state:

- pending plan approval records `plan_ready`;
- approved plan decision records `plan_approved`;
- continue-planning decision records `plan_revising`;
- rejected plan decision records `plan_rejected`.

## Key Decisions

- `PersistedStructuredRolloutEvent::RuntimeState` now carries
  `plan_lifecycle` so checkpoint replacement preserves lifecycle metadata.
- `PersistedStructuredRolloutEvent::PlanLifecycle` remains available as a
  standalone structured event shape for append-only future slices.
- Thread materialization includes lifecycle rollout items so CLI and future
  status surfaces can inspect typed phase data.
- Thread forks preserve materialized planning lifecycle records from the source
  thread instead of resetting lifecycle state.
- Completed plan approvals persist a typed `plan_approval:*` source marker so
  lifecycle decisions do not depend on UI summary copy.
- Session restore uses the latest planning lifecycle phase to recover a pending
  approval card and the `exit_plan_mode` tool-use id. Terminal lifecycle phases
  do not reopen older pending approvals.

## Validation

- `cargo check --locked --workspace --all-targets`
- focused state persistence tests for pending and completed plan approval
  lifecycle checkpoints
- `cargo test tui::session_restore::tests::restore_session_recovers_pending_plan_approval_from_lifecycle`
- `cargo test tui::session_restore::tests::restore_session_does_not_reopen_completed_plan_approval`

## Follow-Ups

- Add `/status` and `/context` planning lifecycle fields.
- Persist user feedback for continue-planning and rejected-plan decisions.
