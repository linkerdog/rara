# Shell Approval Policy

## Problem

RARA allows users to approve reusable bash command prefixes. A reusable prefix is
useful for repeated commands such as `cargo test` or `git push`, but it is also a
safety boundary. A prefix approved for one command segment must not implicitly
approve unrelated segments joined with shell control operators.

## Scope

- Bash tool approval in suggestion mode.
- Reusable command-prefix approvals persisted in RARA config.
- Legacy shell command strings and structured `program` plus `args` calls.
- Read-only command auto-allow classification.

## Non-Goals

- Replacing the current approval UI.
- Implementing a full shell parser.
- Granting external protocol adapters direct approval authority.
- Completing the larger auditable permission and sandbox-bypass rule system.

## Architecture

RARA evaluates bash approval in three layers:

1. Tool input parsing produces a `BashCommandInput`.
2. The agent loop checks plan-mode read-only rules and suggestion-mode approval.
3. The bash tool executes only after the agent loop has allowed the request.

For legacy shell command strings, approval-prefix reuse follows a Codex-style
segment boundary:

- split the command at shell control operators such as `|`, `&&`, `||`, and `;`;
- evaluate each segment independently;
- allow the whole command only when every segment is either read-only or matches
  an approved prefix;
- reject prefix reuse for syntax outside the conservative matcher, including
  redirection, command substitution, environment expansion, wildcard expansion,
  subshell grouping, and leading environment assignments.

Structured `program` plus `args` calls do not need shell segmentation. They use
the normalized program and subcommand prefix directly.

When RARA cannot derive a reusable prefix for a shell command, a user approval
may still be stored as the exact command summary. Exact-command approvals are
replayed only for the same normalized summary. They do not become starts-with
prefix rules and do not cover additional shell segments.

## Control-Plane Readiness

Shell approval remains a runtime policy decision, not a TUI-only shortcut. The
TUI, ACP, Wire, and future protocol adapters should submit structured approval
decisions to the runtime and consume the resulting approval lifecycle events.

RARA remains the authority for evaluating whether a bash request is allowed:

- adapters may present approval choices, but they must not directly mark a tool
  call as allowed;
- prefix and exact-command approvals flow through the same runtime approval
  state used by the local TUI;
- persisted approval rules are loaded into the agent runtime before evaluation;
- reusable prefix matching and exact-command replay are both evaluated by the
  bash request policy helper;
- external adapters cannot bypass segment-level approval checks by sending a
  pre-approved command string.

This keeps future ACP/Wire approval surfaces compatible with the local TUI while
preserving one centralized approval boundary.

## Contracts

- A read-only multi-segment shell command may run without approval in suggestion
  mode.
- A multi-segment command with one approved mutating segment and one read-only
  segment may reuse the approved prefix.
- A multi-segment command with any unapproved mutating segment must stop for
  approval.
- A single approved prefix must never approve an entire shell string solely
  because the first segment starts with that prefix.
- Prefix matching must be conservative. If the command uses syntax that the
  matcher cannot safely model, it must fall back to explicit approval.
- Exact-command fallback must only replay the same full command summary.
- Plan mode remains stricter: non-read-only bash is rejected instead of entering
  the normal shell approval flow.

## Validation Matrix

- Helper tests cover single-segment prefix matching.
- Helper tests cover multi-segment commands with read-only and mutating
  segments.
- Helper tests cover shell syntax that must not use prefix reuse.
- Helper tests cover exact-command fallback for shell commands without a
  reusable prefix.
- Agent-loop tests cover the real suggestion-mode regression where an approved
  `git push` prefix must not allow `git push && rm -rf target`.

## Open Risks

- The matcher is intentionally conservative and can request approval for commands
  that a full shell parser might classify more precisely.
- The larger approval-policy enum, granular approval families, and auditable
  rule provenance remain future work.
- Prefix approvals are still command-prefix based, not full structured command
  policies.

## Source Journals

- `docs/journal/2026-05-04-shell-approval-segments.md`
