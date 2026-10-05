# Transient Notice Lifecycle

## Summary

Issue [#984](https://github.com/linkerdog/rara/issues/984) routes notices through
one application-owned path for redaction, transcript recording, typed severity,
and expiration. The visible contract is
[RUN-08](../interaction/runtime-feedback.md#run-08-typed-redacted-transient-notices).

## Background And Reference Adaptation

Inspected local Codex `f959e7fc9832dfa0ebfb6542ab1bbf829638ac24`:
`codex-rs/tui/src/chatwidget.rs` owns typed information/warning/error history
events; `chatwidget/interaction.rs` owns monotonic temporary footer state while
the bottom pane renders it. Inspected local Claude Code
`4b9d30f7953273e567a18eb819f4eddd45fcc877`:
`src/context/notifications.tsx` uses an eight-second default deadline and guards
replacement notifications against stale timeout callbacks; prompt-input
notifications explicitly choose severity and timeouts.

Adapt application ownership, explicit levels, a finite default lifetime, and
replacement protection. Keep the existing last-notice policy and event-loop
maintenance tick instead of importing a notification queue or global timers.

## Plan And Boundaries

1. Reproduce real settings-dispatch redaction and missing-history failures.
   Establish private notice contents and one publisher before migrating callers.
2. Migrate direct and existing publishing calls with explicit levels. Preserve
   paste ownership and classified bootstrap/OAuth history. Remove duplicate
   history writes and keep continuous heartbeats in runtime progress state.
3. Verify synthetic-clock expiry/replacement, visible severity, idle-loop
   repaint, paste behavior, and full TUI consumers. Review changed rendering
   evidence and run strict lint/format gates before publication.

Only local TUI code/tests/docs and the authorized feature branch/PR change.
No dependency, runtime protocol, persisted schema, or Bazel configuration change
is needed. The two failing settings-dispatch tests provide the code-level
feasibility proof before implementation.

## Implementation Decisions

- `state/notices.rs` is the only owner of mutable notice contents. Public TUI
  accessors are read-only; every publishing call supplies `NoticeLevel`.
- Redact before creating either notice state or its transcript entry. Preserve
  classified bootstrap/OAuth records through the same publisher. Expiration
  leaves the record intact and reports whether a frame is needed.
- Each replacement carries its own monotonic deadline. The existing 166 ms
  maintenance tick clears expired notices and requests the coalesced frame;
  no delayed timer can target a replacement's predecessor.
- Paste flushing reports its feedback to the application. Clearing the draft
  removes only a paste-owned notice, independent of text equality. Clipboard
  outcomes carry typed information/warning/error feedback before publication.
- Heartbeats update existing runtime progress without generating a new notice
  every second. Completion records join the completed turn before finalization.
- Keep the existing distinction between routine system records and renderable
  classified diagnostics. Query/catalog/compaction errors, compaction results,
  and bootstrap/OAuth feedback retain a single classified record. Routine
  records alone do not hide startup chrome or a pending planning prompt.
- Resume, clear, and queued-query tests assert their new system record explicitly;
  live and committed storage retain redacted notices after the footer expires.
- Credential-save handling moves into a focused event-dispatch module to keep
  the touched source below the 1000-line limit.

## Validation

Two original-code regressions fail behaviorally: a saved base-URL notice leaks
its synthetic password, and its transcript record is absent. Both pass after
the initial publisher and caller migration.

Eleven new tests cover settings dispatch, exact deadline boundaries, replacement,
paste ownership, classified history, persisted redaction, typed status colors,
clipboard outcomes, query failure rendering, and idle-loop repaint. Existing
heartbeat tests also assert that progress neither replaces a warning nor adds a
transcript record. Existing pending-interaction and busy-status tests cover their
priority over errors; corrupt-goal restore asserts warning severity.

Validation evidence:

- `cargo test --locked -p rara --lib tui::`: 1056 passed, four existing ignored.
- `cargo clippy --locked --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all` and `git diff --check`: passed.
- Existing snapshots remain unchanged. The first full suite exposed startup and
  planning assumptions about empty transcript state; those integration regressions
  were corrected before the passing run. Fixture expectations now include the
  single recorded notice without relaxing their user-entry or stream assertions.
- No remaining `bottom_pane.notice` writers or string-prefix severity checks;
  all touched Rust sources are below 1000 lines.

Local default Bazel has the previously recorded external-cache failure resolving
`rules_rust//rust`; its configuration/cache was left unchanged. Remote default
Bazel remains the integration gate. No physical-terminal manual run was added;
existing production-loop and terminal-emulator tests cover expiration repaint.

## Follow-Ups

No implementation follow-up remains within #984. Complete remote CI/review before
integration. Notice severity/deadlines are presentation state; persisted records
retain the existing role/message format, and routine system records retain their
existing non-card presentation.
