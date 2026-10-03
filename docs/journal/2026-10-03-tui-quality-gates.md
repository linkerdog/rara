# TUI Terminal Oracles And Lint Gates

## Summary

This first #927 checkpoint consolidates the terminal regression surfaces added for
#920 through #926 and installs TUI-specific printing and color lint gates.
The stable quality contract is QUALITY-05 in the interaction verification spec.

## References And Plan

Inspected Codex at `ea2046f36d5ee12d39c8e168fc3e5129301afa2b`:
`tui/src/lib.rs`, workspace `clippy.toml`, `tui/src/test_backend.rs`,
`tui/tests/suite/vt100_history.rs`, and `tui/src/tui/frame_requester.rs`.
Inspected Claude Code at `4b9d30f7953273e567a18eb819f4eddd45fcc877`:
`src/ink/ink.tsx` render scheduling and console/stderr interception.

Reuse the existing vt100-backed terminal, event injection, and explicit frame
instants. Codex's compile-time print/color restrictions fit this Rust surface;
Claude's stderr interception documents why out-of-band writes corrupt a
diff-rendered frame. Use warning logs instead of installing process-global
stream patches. Keep configurable theme and syntax conversion exceptions.

Stage one maps issue examples to existing guards and adds a real terminal-output
wide-cell regression. It exits once a minimal diff mutation is detected.
Stage two enables lint gates, proves existing renderer prints and temporary
color mutations are rejected, repairs actual violations, and validates the
final branch. This checkpoint adds no new CI job, event-loop abstraction,
dependency, or Bazel configuration. The separate production-loop seam requested
in the #938 review remains part of #927, so this checkpoint does not close it.

## Changes And Regression Evidence

- The TUI module denies direct stdout/stderr print macros and configured color
  methods. Two existing renderer stderr diagnostics now use warning logs.
- Theme ownership and two syntax conversion functions carry documented lint
  expectations. Test-only diagnostics and PTY protocol markers have scoped
  expectations that become warnings if the exception is no longer needed.
- The existing vt100 backend now checks narrow/wide replacements and attribute
  reset across real differential terminal writes. Removing continuation-cell
  skipping produced `ab\u{4e2d} ail!` instead of `ab\u{4e2d}tail!`; restoring
  the production diff restored the expected screen.
- Clippy rejected both existing renderer stderr sites, an inserted stdout print,
  and each of the five configured raw-color/shortcut mutations. All temporary
  mutations were removed before final validation.
- Full-suite verification also exposed a child-reaping defect in the parent
  clipboard change. The fix and unchanged reaping bound were validated and
  pushed to #948 before this checkpoint resumed; see its implementation journal.
- An existing plan-mode test failed under Bazel load because its 20 ten-millisecond
  polls expired before the task returned; the automatic retry passed. Both
  adjacent plan lifecycle tests now await each real task join and pass its
  completion through the unchanged production handler, including follow-up
  tasks. A five-second outer deadlock bound replaces timing as an oracle;
  all original plan and approval assertions remain intact.

## Validation

- `cargo test --locked --lib tui:: -- --nocapture`: 919 passed, three ignored
  subprocess fixtures exercised by their parent tests.
- `bazel test //:rara_unit_tests --test_arg=tui::`: 919 passed after the plan
  fixture synchronization fix, with no retry in the final run. The earlier
  missing external rules cache was restored with a scoped default-target fetch.
- `cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings`:
  passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.

## Remaining Limits

The print gate does not intercept direct `Write` calls or dependency output.
The color baseline does not prohibit every named ANSI color or establish a
state/presentation dependency boundary. Physical terminal, SSH, tmux, and macOS
acceptance remain distinct from deterministic terminal emulation and Unix PTY
tests. The legacy root panic-lint allows are unchanged.

The current input harness and explicit scheduler instants are component seams,
not execution of `run_event_loop`. Production loop injection and frame wiring
coverage remain in #927 and `docs/todo.md` as required by the #938 review.
