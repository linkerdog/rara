# Sandbox Denial Escalation Guidance

## Summary

Strengthened RARA's runtime prompt and `bash` tool guidance so sandbox-denied
validation work follows the same contract Claude Code uses: treat denials as
new information, do not blindly repeat the exact same call, and escalate with
concrete justification when the blocked capability is essential.

## Background

RARA already told the model not to give up on tests, builds, or checks just
because the first sandboxed command failed. In practice that still left one
important gap: the model could see a denial, report it, and then stall without
either narrowing the command or requesting `require_escalated`.

Claude Code spreads the missing behavior across its harness and denied-tool
prompts:

- a denied tool call is not something to repeat verbatim;
- the model should adjust its approach based on why the call was denied;
- when the blocked capability is necessary, the model should explain that need
  and request the required permission.

This change ports that contract into RARA's own prompt-locality model instead
of copying Claude-specific wrappers.

## Scope

- Added always-on prompt guidance that denied tool calls, denied sandboxed
  commands, and denied escalation requests must not be retried verbatim.
- Added execute-mode guidance that verification todos stay pending when
  validation remains blocked by denied commands or denied escalation.
- Expanded the `bash` tool description and input schema so the call site itself
  teaches: inspect the denial, narrow the command, and prefer
  `require_escalated` over repeating an already denied essential validation
  command.
- Tightened CI lockfile discipline by running the existing `clippy` and `test`
  workflows with `--locked`, so lockfile drift fails normal PR validation
  instead of showing up only in release jobs or local hooks.
- Updated the prompt-runtime spec to document denied-call handling as part of
  sandbox-escalation locality.

## Key Decisions

- Keep the broad "denials are routing information" rule in the always-on prompt
  because it applies beyond any single tool.
- Repeat the sandbox-specific escalation rule in `bash` because the model needs
  it at the exact moment it chooses whether to retry or request approval.
- Tie denied validation back to `todo_write` only in execute mode so plan and
  review turns stay read-only and narrower.

## Validation

- `cargo fmt --all`
- `cargo test -p rara-instructions`
- `cargo test bash_tool_schema_guides_command_discipline`

## Follow-Ups

- No new follow-up was opened by this slice.
