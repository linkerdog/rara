# Verify Skill

## Problem

RARA has testing and validation guidance in the default prompt, but it does not yet have a reusable
verification workflow that can be invoked when the user asks to verify a change or when the agent is
near completion on a user-visible behavior change.

Claude Code treats verification as a skill-driven workflow: `verify` establishes the verification
stance, then repo-local `verifier-*` skills define the project-specific evidence-capture protocol.
RARA should adopt that shape before considering a runtime-level verifier tool.

## Scope

- A repo or home skill named `verify`.
- Repo-local verifier skills named `verifier-*`.
- A SkillTool-driven invocation flow.
- Guidance for runtime-surface verification, evidence capture, and structured reporting.
- Integration points for future `/review`, `/context`, and `/status` visibility.

## Non-Goals

- Adding a built-in `verify` runtime tool.
- Replacing lifecycle hooks with verifier skills.
- Replacing unit tests, type checks, format checks, or CI.
- Automatically installing browser, tmux, API, or recorder dependencies.
- Treating verifier skills as permission to mutate files or external systems.

## Architecture

### 1) Skill Shape

The primary skill should live as ordinary skill content:

- repo-local: `.agents/skills/verify/SKILL.md`
- home-level: `~/.rara/skills/verify/SKILL.md` or `~/.agents/skills/verify/SKILL.md`

The skill's frontmatter should identify it as:

```yaml
---
name: verify
description: Verify that a code change works through the real user-facing or programmatic surface.
---
```

The body should teach the agent to:

- identify the actual changed surface from the diff;
- prefer runtime observation over re-running CI-style tests;
- find a matching repo-local `verifier-*` skill before inventing a workflow;
- drive the smallest path that executes the changed behavior;
- capture evidence from the running app, CLI, API, TUI, or agent surface;
- report `PASS`, `FAIL`, `BLOCKED`, or `SKIP`.

### 2) Verifier Skills

Project-specific verifier skills should be normal skills with names that start with `verifier-`:

- `.agents/skills/verifier-cli/SKILL.md`
- `.agents/skills/verifier-tui/SKILL.md`
- `.agents/skills/verifier-api/SKILL.md`
- `.agents/skills/verifier-web/SKILL.md`

The `verify` skill should look for these first and invoke the best match through `SkillTool`.

Verifier skills are the project's evidence-capture protocol. They may describe:

- how to start the app or test harness;
- how to reach the changed surface;
- how to capture terminal output, screenshots, API responses, or logs;
- what local limitations or required credentials exist;
- what counts as `PASS`, `FAIL`, `BLOCKED`, or `SKIP`.

### 3) Invocation Flow

The expected flow is:

1. User asks to verify, validate, manually test, confirm a fix, or check a PR behavior.
2. Agent invokes `skill` with `skill_name = "verify"` if available.
3. The `verify` skill inspects the diff and available skills.
4. If a matching `verifier-*` skill exists, invoke that skill before running the verification.
5. Agent executes the verification through normal RARA tools.
6. Agent reports the verdict and evidence.

The first implementation should remain advisory. The runtime should not force every task through
`verify`, because project Stop hooks and verifier skills serve different purposes: the former
mechanically gate completion while the latter guide the agent's evidence-gathering workflow.

### 4) Report Format

Verifier output should be concise and evidence-backed:

```markdown
## Verification: <surface or behavior>

**Verdict:** PASS | FAIL | BLOCKED | SKIP

**Claim:** <what the change is supposed to do>
**Method:** <how the changed surface was reached>

### Evidence
- <command, URL, screenshot, API response, or terminal output reference>

### Findings
- <runtime observation or failure, if any>

### Unverified
- <what was not checked and why>
```

`SKIP` is valid for docs-only, tests-only, type-only, or other changes with no runtime surface.
`BLOCKED` means the verifier could not reach an observable surface.

## Contracts

### SkillTool Contract

- `verify` is invoked through the same `skill` tool as other skills.
- The agent must not invent `verify` or `verifier-*` names that are not present in the available skill list.
- If no `verify` skill exists, the agent may perform ordinary validation from the default prompt but
  must not claim it followed the verify skill.
- If a matching verifier skill exists, the agent should invoke it before starting runtime
  verification.

### Evidence Contract

- Verification evidence must come from the real surface whenever possible.
- Unit tests in the diff are author evidence, not verifier evidence.
- Re-running tests can be useful for implementation validation, but it is not enough to satisfy a
  runtime verification request unless the changed surface is itself a test-only surface.
- The report must distinguish observed behavior from inferred behavior.

### Safety Contract

- Verifier skills cannot bypass RARA sandbox, approval, tool, or permission rules.
- If verification requires network, credentials, external systems, or destructive actions, the agent
  must follow the normal approval and safety flow.
- Verifier skills should be treated as local instructions, not trusted executable code.

## Validation Matrix

- Skill discovery tests for `verify` and `verifier-*` names.
- Prompt tests proving available skills tell the model to invoke matching skills before acting.
- SkillTool tests proving unknown verifier names are rejected.
- Manual verification of a small local CLI/TUI change using `verify` once the skill exists.
- Future TUI/status tests showing invoked verifier skills in context once skill invocation tracking
  is implemented.

## Open Risks

- RARA currently lacks a stop-condition verifier hook, so `verify` cannot enforce final quality by
  itself.
- Without skill invocation tracking, `/context` cannot yet explain which verifier was used after
  compaction.
- Verifier skills may go stale. The report should classify stale verifier mechanics as `BLOCKED` or
  ask whether to update the verifier, not fail the product change automatically.

## Source Journals

- [2026-05-01-skilltool-and-verify-specs](../journal/2026-05-01-skilltool-and-verify-specs.md)
