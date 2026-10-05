# Runtime Diagnostics And Terminal Ownership

## Summary

Issue #987 identified library stderr writes during TUI ownership. The affected
paths include live transcript recovery, turn indexing, file-read bookkeeping,
runtime initialization, shell environment capture, and background memory work.
The CLI now installs a diagnostic receiver before runtime assembly. During TUI
ownership it routes warning/error records into an application-owned queue for
normal notice and transcript rendering. Runtime libraries use `log`.

## Decisions

- Inspection found no installed `log` receiver. Replacing print macros alone
  would suppress diagnostics, so receiver ownership is part of this fix.
- Local Codex `f959e7fc9832dfa0ebfb6542ab1bbf829638ac24` uses separate diagnostic
  sinks in `codex-rs/tui/src/lib.rs`; rollout recovery in
  `codex-rs/rollout/src/recorder.rs` returns parse-error counts alongside records.
  Claude Code `4b9d30f7953273e567a18eb819f4eddd45fcc877`
  (`src/utils/log.ts`) queues errors for its sink and retains in-memory errors.
- The CLI owns the process logging facade. Its only global mutable routing
  reference is weak and points to a diagnostic queue, never an agent/runtime
  handle. One terminal owner captures all process-thread warnings/errors.
  Embedding runtime APIs do not initialize or replace the host logger.
- Capture starts before raw mode and ends after terminal restoration, including
  unwind/error paths. A second terminal owner is rejected. Pending records left
  during shutdown go to stderr after restoration; protocol stdout is untouched.
- Redact before queueing; cap each message at 8 KiB and the queue at 256 records.
  Retain recent records and report overflow. Consecutive duplicates remain
  coalesced across drains so failed diagnostic persistence cannot loop forever.
  Diagnostics already contained in the current same-severity recovery notice
  do not replace or duplicate that combined warning.
- Live recovery returns valid entries plus bounded recovery information. The
  compatibility loader logs warnings; TUI restore combines them with its other
  resume warnings. Malformed and invalid UTF-8 lines are skipped, while an I/O
  failure ends the read with its valid prefix retained. Reading never rewrites
  the original log.
- A StateDb index failure still leaves a successful canonical turn append
  successful. Log the indexing failure rather than inviting a duplicate commit.
- Workspace Clippy denies print macros. Explicit CLI/protocol consumers and
  isolated test fixtures retain documented exceptions. `rara-tools` adds the
  already-resolved `log` dependency; no dependency version or storage schema changes.

## Validation

- An isolated TUI harness process reproduces the old stderr corruption from
  both a malformed live log and an injected SQLite index failure.
- Recovery tests cover malformed/invalid-UTF-8 lines, a truncated tail, empty
  and missing logs, open errors, and an injected mid-read error.
- Diagnostic tests cover background-thread delivery, history/rendering,
  redaction, count/size bounds, visible overflow, repeated persistence failure,
  logger handoff and exclusive ownership. Restore tests retain valid live
  entries alongside existing approval recovery warnings.
- Eleven new focused tests pass. Full TUI validation passes 1066 tests; seven
  isolated subprocess entry points are marked ignored, including three new
  entries exercised by their parent tests. Existing snapshots are unchanged.
- The persistence crate passes all 11 tests. Strict workspace/all-target Clippy,
  formatting, and diff checks pass. The actual app-server binary passes normal,
  EOF, malformed input, truncated input, and output-loss scenarios.
- Commands: `cargo test --locked -p rara-persistence`,
  `cargo test --locked --lib diagnostics`, `cargo test --locked --lib tui::`,
  `cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings`,
  `cargo fmt --all`, `git diff --check`, `cargo build --locked --bin rara`, and
  `python3 scripts/app_server_smoke.py target/debug/rara`.

## Integration

This change is stacked on #1037 and transitively #1035 to retain typed notices
and approval recovery. When integrating #1029, carry live recovery information
through the prepared restore and keep diagnostic draining in the UI owner.
The CLI logger observes process diagnostics; it does not change session policy
or task admission. Embedded runtime and app-server APIs leave logging
initialization to their host.
