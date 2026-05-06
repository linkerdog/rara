---
name: add_tests
description: Add or update tests for the RARA codebase. Use when implementing non-trivial behavior changes, fixing regressions, tightening TUI rendering, or adding repo-specific test coverage in Rust. Especially relevant for focused unit tests, TUI snapshot tests, agent/runtime regressions, and behavior-driven test updates in this repository.
---

# Add Tests For RARA

Use this skill when a change in `rara` needs test coverage or when an existing failing behavior should be captured with a regression test.

## Goals

- Prefer focused, behavior-driven tests over broad integration churn.
- Capture the real regression first, then add only enough setup to make the failure explicit.
- Keep tests aligned with the repository's TUI, runtime, and local-first architecture.

## Default Testing Strategy

For a non-trivial change, choose the narrowest useful test surface:

1. Pure helper logic:
   - Add a unit test next to the helper.
   - Prefer direct function assertions.

2. TUI event or state transitions:
   - Add tests in `src/tui/runtime/events.rs`, `src/tui/state.rs`, or the nearest focused module.
   - Assert on structured state, not incidental strings, unless the visible text is the contract.

3. TUI rendering contracts:
   - Use focused render tests in `src/tui/render/*`.
   - Use snapshot tests when layout, ordering, labels, or visual grouping are the contract.
   - Keep snapshots stable and intentionally small.

4. Agent / planning / transcript regressions:
   - Add tests that reproduce the exact transcript or state-routing bug with the minimum number of events.
   - Prefer asserting the object-level result first, then render output if the bug is presentation-specific.

5. Config / persistence behavior:
   - Use `tempdir()` and isolate filesystem state.
   - Never rely on the current working directory or developer-local paths.

## Repository-Specific Guidance

### Assertion Style

- Prefer object-level equality assertions when the whole value is the contract.
- Use field-by-field assertions only when each field communicates a distinct
  behavior or failure mode.
- Avoid assertions that only check debug text when the same behavior can be
  asserted through structured state or typed events.
- Avoid mutating global process environment in tests. Prefer passing
  environment-derived inputs through constructors, config structs, or small test
  doubles.

### TUI Tests

Common places:

- `src/tui/runtime/events.rs`
- `src/tui/state.rs`
- `src/tui/render/cells.rs`
- `src/tui/render/bottom_pane.rs`

Patterns:

- Build a `TuiApp` with `ConfigManager { path: temp.path().join("config.json") }`.
- Feed `TuiEvent` values through `apply_tui_event(...)` when testing runtime/event routing.
- For rendering, build the relevant cell and collect rendered lines with `.display_lines(width)`.
- If the UI shape is the contract, prefer `insta::assert_snapshot!`.

### Planning / Transcript Bugs

When fixing plan-mode bugs:

- Assert that the wrong section stays empty if that is the regression.
  - Example: planning prose should not populate `exploration_notes`.
- Assert the intended destination explicitly.
  - Example: notes land in `planning_notes`.
- If there is a visible card contract, add a render test for it.

### Snapshot Tests

Use snapshots for:

- `Updated Plan`
- `Awaiting Approval`
- model picker overlays
- queued follow-up previews
- other transcript-heavy cards where ordering and grouping matter

Do not use snapshots when a short structural assertion is enough.

When a snapshot changes:

- Review the generated snapshot text before accepting it.
- Confirm that the visible ordering, labels, grouping, and truncation behavior
  match the intended user-visible contract.
- Do not accept snapshots only to make CI green.

### Runtime / Protocol Tests

For agent-loop, provider, Wire/ACP, or tool-result tests:

- Assert structured request or event payloads where possible instead of
  substring-matching serialized JSON.
- Keep fixtures close to the behavior under test and avoid broad provider mocks
  when a narrow fake backend is enough.
- If a regression depends on transcript order, assert the ordered typed entries
  first, then add rendered text or snapshot coverage only when the UI contract is
  also affected.

## What Good Tests Look Like Here

A good `rara` test usually has these properties:

- It names the regression in the test name.
- It uses the smallest realistic fixture.
- It avoids unrelated providers, tools, or file IO.
- It checks the behavior that matters, not every internal detail.
- It remains readable without reverse-engineering a giant setup block.

## Common Mistakes To Avoid

- Do not add broad end-to-end coverage for a local helper change.
- Do not depend on machine-local paths, current repo layout, or ambient environment.
- Do not overfit to debug text if the real contract is a state transition.
- Do not update snapshots blindly; verify the new shape is actually intended.
- Do not mix multiple unrelated regressions into one test.

## Typical Commands

Run the narrowest relevant tests first.

Examples:

```bash
cargo test tui::runtime::events::tests -- --nocapture
cargo test tui::render::cells::tests -- --nocapture
cargo test agent::tests::completes_only_active_plan_step_on_finish -- --nocapture
cargo check
```

If a change only affects one rendering contract, prefer one focused test target plus `cargo check`.

## Heuristics For Choosing Assertions

Prefer this order:

1. State assertions
2. Structured transcript assertions
3. Rendered text assertions
4. Snapshot assertions

Use the highest level that still captures the bug precisely.

## When To Add A Snapshot

Add or update a snapshot when:

- the user-visible layout is the contract
- multiple sections must stay in a stable order
- labels, grouping, or card boundaries matter
- a previous bug involved the wrong surface being shown

## Minimal Workflow

1. Reproduce the bug with the smallest focused test.
2. Make the test fail for the right reason.
3. Implement the fix.
4. Re-run the focused test.
5. Run `cargo check`.
6. Only broaden test coverage if the change truly crosses module boundaries.
