# Engineering Guidance Placement

## Summary

RARA now records a clearer split between always-on runtime prompt rules, tool
description rules, repository engineering conventions, and task-specific skills.

## Background

Codex and Claude Code both carry software-engineering guidance, but they place
different rules at different layers. Broad agent behavior belongs in the runtime
prompt, call-time tool constraints belong in tool descriptions, and repository
maintenance conventions belong in workspace instructions or skills.

## Scope

- Added a small runtime prompt section for software-engineering task framing.
- Added terminal Markdown output guidance to the runtime prompt.
- Kept RARA-specific SDD, journal, and TODO rules out of the default runtime
  prompt.
- Added Rust, TUI, prompt-placement, and testing conventions to repository
  guidance.

## Key Decisions

- The default runtime prompt may tell the model to interpret terse repository
  requests in the current workspace context.
- User-facing assistant text is terminal-rendered GitHub-flavored Markdown, so
  prompt guidance should prefer concise structure, language-tagged code fences,
  `path:line` references, and no emojis unless requested.
- The default runtime prompt must not encode RARA-only documentation workflow
  terms such as SDD or journal requirements.
- `AGENTS.md` is the right home for RARA repository maintenance rules.
- Test-skill guidance is the right home for deeper assertion, snapshot, and
  protocol-test expectations.

## Validation

- Focused prompt tests should assert that the new software-engineering section
  is present.
- Focused prompt tests should also assert that RARA-only SDD terminology is not
  injected into the default runtime prompt.
