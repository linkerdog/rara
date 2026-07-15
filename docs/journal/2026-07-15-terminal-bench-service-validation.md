# Terminal-Bench Service Validation

## Summary

The `terminal-bench/headless-terminal` trial completed normally but received
Harbor verifier reward `0.0`: six checks passed, while the background HTTP
service was not reachable from the verifier. The Harbor adapter now requires
independent-client validation for background processes, daemons, and network
services.

## Scope

- Add task-agnostic benchmark guidance for externally observable service
  behavior.
- Preserve the core default prompt and runtime/tool behavior.
- Add focused adapter coverage for the new guidance.

## Key Decision

Service launch output, a PID, and a process listing only establish shell-local
state. A task that requires a long-running service must be validated from a
fresh client process after startup.

## Validation

- `PYTHONPATH=$PWD/tools/harbor /home/hawkingrei/.local/share/uv/tools/harbor/bin/python -m unittest tools.harbor.test_rara_agent`
- `git diff --check`

## Follow-Ups

- Re-run `terminal-bench/headless-terminal` with the updated adapter and record
  the Harbor verifier artifact in the observed-results journal.
