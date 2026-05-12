---
name: project-github-titles
description: Use when preparing commit messages, pull request titles, or first-line summaries for the RARA repository. Enforces the repo's short conventional title subset using feat, fix, chore, or test.
---

# Project GitHub Titles

Use this skill when writing commit titles, pull request titles, or the first
summary line in PR/commentary text for this repository.

## Title Format

Use one of:

- `type: subject`
- `type(scope): subject`

Never prefix titles with `[codex]`.

Keep title text in English, concise, and specific. Do not end the subject with a
period.

## Allowed Types

RARA intentionally uses a small subset:

- `feat`: user-visible feature or capability
- `fix`: bug fix or behavior correction
- `chore`: maintenance, dependency, tooling, docs, or non-user-facing cleanup
- `test`: test-only changes

Do not use unlisted types such as `docs`, `refactor`, or `style`. Map those to
the closest allowed type, usually `chore` for docs or maintenance.

Do not use `!` breaking-change markers in titles. Describe compatibility impact
in the PR body instead.

## Scope Rules

Use a scope only when it clarifies the primary touched area. Keep it short and
lower-case.

Useful scopes in this repository include:

- `agent`
- `tui`
- `tools`
- `memory`
- `context`
- `provider`
- `plugins`
- `skills`
- `acp`
- `wire`
- `ci`

Prefer stable product or domain scopes over file names.

## Subject Rules

- Describe the main effect, not the mechanics.
- Prefer imperative or result-oriented phrasing.
- Avoid vague subjects such as `update stuff` or `misc cleanup`.
- Use lower-case unless a proper noun or code identifier requires otherwise.

Good examples:

- `fix(tui): keep approval overlay above transcript`
- `feat(memory): add local embedding prototype`
- `chore(skills): add RARA journal writing skill`
- `test(agent): cover duplicate tool call warning`

## PR Body

PR descriptions should use markdown and include:

- summary of changes
- purpose or motivation
- related issue, if any
- testing or validation

For documentation-only changes, `Testing: Not run; documentation-only change.`
is acceptable.
