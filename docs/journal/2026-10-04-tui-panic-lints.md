# Scoped TUI Panic Lint Enforcement

## Scope And Reference Review

Issue #976 found that the root library's legacy lint exemptions also disabled
panic-prone Clippy checks in the TUI. The scoped source-module gate restores
production checks without broadening the root-crate migration in #871.

Reviewed Codex at `f959e7fc`: `codex-rs/tui/Cargo.toml` inherits workspace
`unwrap_used`/`expect_used` denies; its `clippy.toml` exempts test assertions.
The local reference does not deny every panic macro. Reviewed Claude Code at
`4b9d30f7`: `src/components/SentryErrorBoundary.ts` isolates failed component
rendering and `src/cli/transports/SSETransport.ts` catches transport errors.
Adapt the scoped checks and explicit failure handling, not a UI-wide catch-all
or a promise that linting prevents all panics.

## Implementation Plan

1. Define production lint scope in the interaction quality contract and use
   Clippy to inventory violations against that gate.
2. Replace avoidable option assertions and unreachable branches with structural
   matches or propagated errors. Review remaining task-assembly invariants
   against their runtime ownership journal and annotate only justified items.
3. Verify negative production probes for every lint, focused state transitions,
   all-targets strict Clippy, formatting, and existing CI boundaries.

## Implementation

The production TUI module now denies `unwrap_used`, `expect_used`, `panic`,
`todo`, `unimplemented`, and `unreachable`. Source attributes apply independently
of Cargo/Bazel configuration. Test fixtures and panic injection remain exempt;
the normal library target still checks production code during all-targets
Clippy. No root-crate allowance or Bazel configuration was changed.

The initial inventory found 21 sites. Thirteen were removed through option
matching, a single goal-rendering match, error propagation, and direct ownership
transfer. A goal cleared before its initial projection now returns a recoverable
error. Repository detection consumes only a finished handle and logs join failure.

Eight sites remain behind six item-level expectations with reasons:

- Two fixed local-link regex initializers, exercised by location-format tests.
- Query task construction's three required runtime handles and the review task's
  required event bus; `run_tui_session` installs them before input dispatch.
- The legacy service adapter's production panic: the processor supplies services
  to real commands and completions, while compatibility fixtures assemble them
  from test-only fields. Its existing transitional callers prevent simply
  marking the whole adapter test-only; see the runtime task-service ownership
  journal dated 2026-08-03 and the broader #871 migration.
- The private query-completion state transition: the owning task calls it exactly
  once after execution returns. Stop/return interleavings retain existing tests.

The expectations are scoped to the individual static or function and are checked
for fulfillment. They do not suppress future violations across a file or module.

## Validation

- Enabling the gate on the original code produced 21 Clippy errors.
- A temporary production probe produced all six named Clippy errors. The probe
  was removed afterward; test-build exemptions did not weaken the normal library
  target.
- Replacing the new goal-start check with the old `expect("new goal")` made
  `goal_cleared_before_start_returns_a_recoverable_error` panic. The fixed check
  was restored before final validation.
- Source was synchronized to main after #972, #973, and #974 merged. The approval
  preview conflict retained the new scrollable shell panel and used an optional
  last-row update only in the non-shell preview.

Final commands:

```bash
cargo test --locked -p rara --lib tui::
cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings
cargo fmt --all
```

The TUI suite reports 1018 passed and 4 existing ignored tests; strict workspace
Clippy passes for all targets. The existing local Bazel external cache lacks `rules_rust//rust`; remote default Bazel CI remains the merge gate.

## Follow-Ups

The six documented invariant exceptions and the root crate's broader allowances
remain explicit limits, not a claim of a panic-free TUI. Removing compatibility
service plumbing belongs to the existing #871 runtime migration. No additional
issue work is deferred from #976.
