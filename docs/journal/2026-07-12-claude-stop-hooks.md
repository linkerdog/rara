# Claude-Compatible Stop Hooks Checkpoint

## What Changed

RARA now discovers project `hooks.Stop` command handlers from
`.claude/settings.json` and executes them when the agent would otherwise finish
without tool calls. A handler receives JSON context on stdin and runs in the
workspace. Exit code `2`, or Claude-style JSON `{ "decision": "block" }`,
keeps the agent loop alive and feeds the failure reason back into the next model
turn.

## Why

Completion requirements must be enforced through visible project contracts,
not by reading a benchmark's private verifier. This follows Claude Code's Stop
hook model: projects can require a check before completion while the agent gets
the failed check's diagnostic and can repair its work.

## Safety And Compatibility

The initial implementation supports only command handlers under `hooks.Stop`.
Handlers run in the workspace, have an optional per-hook timeout, and receive
only session and workspace context. RARA treats exit code `2` as a blocking
result; other non-zero exits and timeouts are warnings, which preserves Claude
Code's distinction between enforcement failures and hook execution failures.

To avoid endless retries, RARA mirrors Claude Code's bounded continuation
behavior and allows at most eight consecutive Stop-hook blocks before ending
the turn with a visible error event.

## Follow-Up

Expand the compatibility surface deliberately: matcher-aware tool hooks,
structured command output beyond the Stop subset, and non-command handlers need
separate security and timeout contracts.
