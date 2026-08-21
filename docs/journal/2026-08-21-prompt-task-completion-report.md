# Require Task-Completion Report in Default Prompt

## What

Added one rule to the `Task Workflow` section of the default prompt in
`crates/instructions/src/prompt.rs`:

```text
When you complete the task, respond with a concise report covering what was
done, the exact validation result, and any remaining follow-up or next step.
Do not end on a bare confirmation such as `done` or `finished`.
```

Also added a `task-completion reporting` bullet to the built-in engineering
workflow guidance list in `docs/features/prompt-runtime.md`, plus a focused
assertion in `crates/instructions/src/prompt/tests.rs`.

## Why

The default prompt constrained task start and in-progress behavior but never
required a closing statement. `Task Workflow` said `verify, then report`
without defining `report`, and the agent loop treated "no tool call" as the
final answer mechanically. The model therefore ended on one-word confirmations.

The wording mirrors Claude Code's `DEFAULT_AGENT_PROMPT`
(`When you complete the task, respond with a concise report covering what was
done and any key findings`) and Codex's `Presenting your work and final
message` section.

## Trade-offs

- The rule is placed additively at the end of `Task Workflow` to preserve
  provider cache-prefix stability (per the prompt-section ordering rule).
- No new prompt section was added; the rule is scoped to the existing section.

## Remains

- Nothing for this change.
