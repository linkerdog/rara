---
name: verify
description: Verify that a code change works through the real user-facing surface.
---

# Verify

When the user asks to verify, validate, manually test, confirm, or check a behavior
change, follow this workflow:

## 1. Identify the Changed Surface

- Read the diff to understand what changed.
- Determine the observable behaviour boundary: what does the user or caller actually see?

## 2. Find a Matching Verifier Skill

- List available skills via the `skill` tool with `action = "list"`.
- Look for skills whose names start with `verifier-` that match the changed surface area.
- If a matching verifier skill exists, invoke it before running the verification.

## 3. Drive the Smallest Verification Path

- Start the app, tool, or harness in the smallest form that exercises the change.
- Prefer runtime observation over re-running CI-style tests.
- Use real CLI invocations, API calls, TUI rendering, or browser output as appropriate.

## 4. Capture Evidence

Evidence must come from the real surface when possible: terminal output, API responses,
TUI snapshots, or log output. Unit tests in the diff are author evidence, not verifier evidence.

## 5. Report

```
**Verdict:** PASS | FAIL | BLOCKED | SKIP
**Claim:** <what the change is supposed to do>
**Method:** <how the changed surface was reached>

### Evidence
- <command, URL, screenshot, API response, or terminal output>

### Findings
- <runtime observation or failure, if any>

### Unverified
- <what was not checked and why>
```

- `PASS`: observed behaviour matches the claim.
- `FAIL`: observed behaviour contradicts the claim.
- `BLOCKED`: could not reach the observable surface.
- `SKIP`: docs-only, tests-only, or changes with no runtime surface.
