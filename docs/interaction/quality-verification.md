# TUI Interaction Quality Verification

## Problem

A green helper test can coexist with broken keyboard routing or stale rendered
help. Quality evidence must match the final interaction it claims to protect.

## Scope And Non-Goals

This specification defines evidence for terminal interaction changes. It does
not require React tooling, a new UI framework, full E2E on every PR, or mutation
testing of the whole repository.

## Contracts

### QUALITY-01: Name The Behavior And Its Oracle

For each changed interaction, identify the owning contract ID, trigger/state,
expected visible result, and the cheapest layer that proves it.

| Risk | Appropriate oracle |
| --- | --- |
| Parser aliases or argument boundaries | Parsed command and preserved argument values |
| Swallowed input, wrong overlay, wrong action | Real key mapping and production dispatch, followed by state/render assertions |
| Hidden, clipped, reordered, or misleading content | Production renderer into a Ratatui buffer; focused text/style assertions or reviewed snapshot |
| Queue/cancel/approval ordering | Scripted runtime events and typed commands, followed by visible state |
| Terminal encoding, resize, paste, alternate-screen restoration | PTY/manual acceptance in the affected terminal environment |

State fixtures are allowed; replacing the production renderer with a parallel
test-only implementation is not a rendering oracle. Metadata assertions alone
cannot prove Help, a picker, or approval card is correct.

### QUALITY-02: Show That A Regression Guard Detects The Defect

For a new behavioral guard, record an old-code replay or minimal production
mutation, its expected failure signature, and the restored implementation.
Do not mutate assertions or make a test fail syntactically to manufacture RED
evidence. Record a compilation/dependency failure separately from a behavioral
failure. If execution is blocked, mark RED/GREEN unverified.

### QUALITY-03: Reuse Existing Presentation Boundaries

- Use semantic theme tokens, shared sanitization, and shared width/wrapping
  utilities; see [theme tokens](../features/tui-theme-tokens.md).
- Share derived lists between render and action paths; model-search row
  identity is the first concrete pilot.
- Use `FakeRuntimeClient`/`TuiHarness` for runtime lifecycle checks. Avoid live
  credentials, model downloads, process-global environment mutation, and sleeps
  in focused interaction tests.
- Keep new test modules focused and below the repository file-size limit.

### QUALITY-04: State The Actual CI Boundary

Existing repository workflows execute Cargo tests, strict workspace Clippy,
formatting, and Bazel tests. Focused Rust interaction tests are compiled into
the existing root test target; no additional slow CI job is needed for them.

Suggested local checks for this surface:

```bash
cargo test --locked --lib tui::interaction_tests
cargo test --locked --lib tui::input_ownership_tests
cargo test --locked --lib permission
cargo test --locked --lib tui::render::tests::approval_layout
cargo test --locked --lib tui::command::tests
cargo fmt --all -- --check
cargo check --locked
cargo clippy --locked --all-targets --no-deps -- -D warnings
```

The Bazel root `rara_unit_tests` target includes source and snapshot globs.
Workflow configuration is not proof that a particular remote run passed or
that branch protection requires a job. A Ratatui buffer test is not PTY or
release acceptance.

## Follow-Up Quality Gates

These are proposed follow-ups, not installed gates:

- A monotonic baseline for new raw-color usage and presentation dependencies
  in state/projection modules; resolved baseline entries must disappear.
- A small width/height matrix for command/help/model/approval surfaces,
  including CJK, emoji, multiline paste, and empty results.
- Scripted completion/cancel/approval interleavings and stale-session events.
- Measured redraw/scroll cost for long transcripts, with deterministic work
  counts before wall-clock performance thresholds.
- PTY smoke tests for terminal-specific keys, resize, and terminal restoration
  in a separate acceptance stage.

These checks need concrete defect examples, owners, runtime budgets, and RED
evidence before becoming required jobs. Track the open work in [TODO](../todo.md).

## Source Journals

- [TUI interaction contracts and reference analysis](../journal/2026-09-17-tui-interaction-contracts.md)
- [Input ownership and draft preservation](../journal/2026-09-17-tui-input-ownership.md)
- [Permission controls and approval layout](../journal/2026-09-17-tui-permission-controls.md)
