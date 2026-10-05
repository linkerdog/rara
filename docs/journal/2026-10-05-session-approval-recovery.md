# Session Approval Recovery

## Summary

Issue #986 exposed a permissive wildcard when reading the persisted bash
approval mode. Session restore now accepts the three known strings explicitly
and recovers unknown values to `Suggestion`, with a warning in both the status
notice and transcript. The next runtime snapshot writes the repaired value.

## Decisions

- Local Codex reference `f959e7fc9832dfa0ebfb6542ab1bbf829638ac24`
  (`codex-rs/protocol/src/protocol.rs`, `AskForApproval`) uses explicit enum
  deserialization rather than treating unknown strings as an authorization.
- Local Claude Code reference `4b9d30f7953273e567a18eb819f4eddd45fcc877`
  (`src/utils/permissions/PermissionMode.ts`, `permissionModeFromString`, and
  `src/state/onChangeAppState.ts`) validates restored strings against known
  modes and falls back to its normal permission mode.
- Adapt these patterns with a private `Option` parser and an explicit
  `Suggestion` fallback at the restore boundary. Preserve the existing meanings
  of `once`, `always`, and `suggestion`; no enum or stored schema changes.
- Combine the recovery message with the resume notice and any goal recovery
  warning. Use the typed notice publisher from PR #1035 and avoid echoing the
  invalid value. This PR is stacked on #1035 for that interface.
- Keep explicit Full Access overrides and existing prefix/read-only grants.
  Invalid stored data cannot itself grant Full Access or session approval.

The canonical contract is in
[`shell-approval-policy.md`](../features/shell-approval-policy.md#persisted-mode-recovery).

## Validation

- Focused restore regressions exercise known and malformed values, both startup
  resume routes, agent/UI/persistence agreement, repair on the next resume,
  transcript warnings, combined recovery errors, and explicit Full Access.
- A restored agent receives an unapproved mutating bash request through the
  normal model/tool loop and must leave it pending approval.
- The original implementation failed the direct recovery and real bash
  approval regressions. The latest-thread fixture includes saved history so it
  exercises restoration rather than the empty-session filter.
- Focused results: 19 restore tests (including six new tests), 43 agent planning
  and approval tests, and eight TUI permission tests pass.
- Commands: `cargo test --lib tui::session_restore`,
  `cargo test --lib agent::tests::planning`,
  `cargo test --lib tui::runtime::permissions`,
  `cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings`,
  `cargo fmt --all`, and `git diff --check`.

## Follow-Ups

Retarget the PR to `main` after #1035 merges. The background restore change in
#1029 must preserve this validation and warning at its apply boundary when the
branches are integrated. No additional approval-policy expansion is required
for this issue.
