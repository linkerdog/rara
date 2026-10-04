# Bounded Asynchronous Review Preparation

## Scope And References

Issue #977 reports that `/review` runs synchronous Git commands on the UI task,
drops exit status and stderr, and confuses command failure with no changes.

Reviewed Codex at `f959e7fc`: the TUI review popup sends a typed review target to
runtime execution; `git-utils/src/operations.rs` preserves failed command status
and stderr. Reviewed Claude Code at `4b9d30f7`: `src/utils/gitDiff.ts` executes
Git asynchronously with deadlines and separates bounded details from summaries.
Adapt runtime-owned preparation and explicit Git errors while preserving the
existing staged/unstaged review scope. Do not copy empty-result error fallbacks.

## Long Plan

1. Implement and verify an owned asynchronous Git capture boundary. Input is a
   workspace path; output is a bounded diff or a diagnostic error. Check both
   statuses, concurrently drain stdout/stderr, and terminate/reap on timeout or
   cancellation. Exit when fake/real subprocess tests prove limits and outcomes.
2. Route review through the runtime maintenance command and an independent
   preparation task. Keep the agent in its owner until changes are ready. Gate
   cancellation against completion so no prepared result can bypass a stop.
   Exit when command/completion tests prove responsiveness, failure isolation,
   clean handling, agent retention, and one review start.
3. Verify integration with existing completion, permissions, goal, and rendering
   behavior; run focused and TUI checks, strict Clippy, and formatting. Publish
   the independent fix for CI and preserve the full issue queue.

A blocking-worker wrapper alone would preserve the UI wait and cannot reliably
cancel its child; use async pipes in a spawned task. The work needs ordinary
workspace edits, local tests/subprocesses, and the already authorized feature
branch/PR workflow. No persistence, public protocol, or Bazel setting changes.

## Implementation Checkpoint

- Added a runtime-owned review preparation task. Git capture does not own the
  agent; clean results, errors, and cancellation return to an available runtime
  without sending a model request or charging a goal turn.
- Both Git commands use asynchronous bounded pipes, a shared 10-second deadline,
  and explicit status/UTF-8 errors. Retained output is limited to 256 KiB across
  both diffs and 16 KiB of stderr per command; the prompt keeps 600 lines and
  names truncation. Excess output is drained to preserve the actual exit status.
- The process guard terminates the child and, on Unix, its process group before
  scheduling reaping when capture is dropped. A stopped preparation cannot
  launch a review even when capture completed before the stop was consumed.
- Review preparation rejects competing maintenance/interaction continuations,
  retains normal input queueing, and applies pending permissions before handing
  over the agent. The subsequent review query receives a fresh cancellation
  token. The composer advertises cancellation and displays ongoing activity.

## Validation

- RED: the command regression failed against the synchronous implementation
  because it had already consumed the agent before Git capture succeeded.
- `cargo test --locked --lib tui::`: 1031 passed, 4 existing ignored. Coverage
  includes missing Git, a non-repository, a clean repository, nonzero status with
  partial stdout, oversized stdout/stderr, UTF-8 at the byte cap, process timeout
  and abort cleanup, responsive input, completion/cancellation races, agent
  retention, maintenance admission, pending permissions, and rendered hints.
- `cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings`: passed.
- `cargo fmt --all` and `git diff --check`: passed.

## Remaining Boundaries

The review scope remains staged and unstaged changes; untracked files are not
included. Unix process groups receive cancellation; other platforms terminate
and reap the direct Git child. Native Windows process-group behavior is not
claimed by the Unix subprocess fixtures. The default remote Bazel job remains
an acceptance gate because the existing local external cache cannot resolve
`rules_rust//rust`; no Bazel settings or dependency locks were changed.
