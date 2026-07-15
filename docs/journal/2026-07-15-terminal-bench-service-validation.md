# Terminal-Bench Service Validation

## Summary

The `terminal-bench/headless-terminal` trial completed normally but received
Harbor verifier reward `0.0`: six checks passed, while the background HTTP
service was not reachable from the verifier. RARA's default prompt now requires
independent-client validation for background processes, daemons, and network
services.

## Scope

- Add task-agnostic default-prompt guidance for externally observable service
  behavior.
- Keep the Harbor adapter and runtime/tool behavior unchanged.
- Add focused prompt assembly coverage for the new guidance.

## Key Decision

Service launch output, a PID, and a process listing only establish shell-local
state. A task that requires a long-running service must be started through the
surface under test, readiness-polled, checked by a fresh client request or
connection, asserted against its expected response, and cleaned up.

## Validation

- `cargo test -p rara-instructions default_prompt_includes_factual_verification_rules -- --nocapture`
- `git diff --check`

## Follow-Ups

- Re-run `terminal-bench/headless-terminal` with the updated adapter and record
  the Harbor verifier artifact in the observed-results journal.
