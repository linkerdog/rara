# 2026-05-05 Team Create Runtime

This checkpoint replaces the placeholder `team_create` implementation with a
real foreground aggregation path.

## What Changed

- `team_create` now accepts an array of task contracts with:
  - `name`
  - `instruction`
  - optional `kind`: `general`, `explore`, or `plan`
- Each task is executed through the existing sub-agent runtime instead of
  returning a mocked status.
- The tool accepts at most 8 tasks and runs at most 4 sub-agents concurrently.
- Task payloads are validated before any sub-agent starts, and the returned
  `team_results` preserve the input order.
- The implementation remains foreground-only: the parent tool call waits for
  all child work before returning.
- Unused TUI display helpers for unfinished completed-state badges were removed
  until that surface is backed by structured sub-agent records.

## Alignment Notes

This brings RARA closer to Claude-style delegation by making multi-agent work a
real runtime path rather than a prompt-only placeholder. It deliberately stops
short of the larger Claude-style task model:

- no durable `agent_id` yet;
- no parent-scoped sidechain transcript write yet;
- no background resume, task output, or stop control yet.

Those pieces should be implemented after the transcript and spawn-edge storage
contracts are wired into the runtime.

## Validation

- `cargo test tools::agent::tests -- --nocapture`
- `cargo check`
