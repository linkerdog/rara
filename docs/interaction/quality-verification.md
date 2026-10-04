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
| Terminal encoding, wide-cell diffs, scrollback, resize | Production terminal output through the vt100-backed `EmulatorBackend` |
| OS terminal modes, process-group suspend, error/panic restoration | Isolated PTY child; manual acceptance for terminal-specific policy |

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
cargo test --locked --lib tui::event_loop::loop_tests
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

### QUALITY-05: Keep Rendering Output And Colors At Their Owned Boundaries

The TUI module denies `clippy::print_stdout`, `clippy::print_stderr`, and
`clippy::disallowed_methods`. Diagnostics use the logger so they cannot write
untracked text into the terminal frame. Terminal protocol writes remain owned
by the terminal and clipboard adapters; the print lints do not intercept all
possible `Write` calls or output from dependencies.

The root `clippy.toml` disallows raw RGB/indexed Ratatui constructors and
white/black/yellow `Stylize` shortcuts. Renderers should use semantic theme
tokens. Theme palette constants, configurable color resolution, and the two
syntax-color conversion functions have item-level exceptions with reasons.
Isolated test fixtures may print protocol markers or diagnostics under
function-level lint expectations; surrounding helpers retain the print gate.

These gates use the existing strict Clippy job. A real renderer stderr write
must fail the print gate; a temporary raw-color/shortcut insertion must fail
the color gate. The printing and color baseline is independent of the production panic-lint
boundary below.

### Existing Regression Surfaces

| Protected behavior | Production seam and regression owner |
| --- | --- |
| Terminal scrollback, resize, wide-cell replacement, synchronized output | `testing::terminal_emulator::EmulatorBackend` and `custom_terminal::inline::tests` |
| Error/panic cleanup and Unix suspend/resume | `terminal_modes_tests` and `job_control_tests`, isolated PTY children, including balanced keyboard enhancement entries |
| Enhanced key decoding and legacy fallback | `keyboard_protocol_tests`, real CSI-u and legacy bytes through the isolated terminal fixture and production input translator |
| Input ordering, paste/submit, key release/repeat, focus, selection | `TuiHarness::send_terminal_event`, production translation/dispatch, `paste_input_tests`, `key_control_tests`, `event_stream`, and `clipboard::tests` |
| Frame deadlines and ordered runtime/input projection | `FrameScheduler` unit guards and `event_loop::loop_tests` with paused time and vt100 frame output |
| Cancel/completion admission and final projection | `controller::cancellation_tests` and `runtime::tasks::tests` |
| Wrapped selection and scroll bounds | `render::viewport_tests` and `selection` tests |
| Composer indentation and split terminal controls | `render::bottom_pane_tests`, `display_sanitize`, and `display_boundary_tests` |

The harness injects `crossterm::Event` values directly into production routing;
it does not duplicate key mapping or install a real terminal reader. Explicit
frame instants avoid sleeps in scheduling tests. VT100 output and PTY mode
checks complement Ratatui buffer assertions; they do not establish acceptance
on every physical terminal, SSH setup, or multiplexer.

### QUALITY-06: Exercise The Production Event Loop

The asynchronous `run_event_loop` accepts a terminal event source and a generic
terminal backend. The source owns input-reader lifetime, raw-mode maintenance,
and suspend handoff. Production uses Crossterm and the session's mode guard;
tests inject events without installing a reader or signaling the process group.
Terminal dimensions come from the same backend that receives frame output.

Loop tests must retain the production controller, command processor, scheduler,
event translation, renderer, and select loop. Pause Tokio time and observe real
ANSI frames through `EmulatorBackend`; do not replace the scheduler or manually
reimplement dirty-state propagation. Verify that bursts retain ordered runtime
and input state, a pending frame wakes independently of maintenance/input,
idle maintenance does not repaint, and resize invalidates stale cells even
when a burst ends at the original dimensions. Errors must retain their normal
notice or fatal-session behavior.

Drive confirmed keyboard quit, `/quit`, and query cancellation through the
production select loop. A first Ctrl-C requests cancellation without ending the
loop or dropping the task; confirmed quit stops outstanding work. Session-level
terminal tests must also observe the final shell cursor handoff before terminal
mode restoration, rather than inferring their order from separate helper tests.

The private I/O seam is not a public extension API. These loop tests complement
the isolated PTY tests; a fake suspend callback cannot establish OS job-control
or physical-terminal reflow correctness.

### QUALITY-07: Reject Unguarded Production Panic Sites

Production code in the TUI module tree denies `clippy::unwrap_used`,
`clippy::expect_used`, `clippy::panic`, `clippy::todo`,
`clippy::unimplemented`, and `clippy::unreachable`, overriding the root crate's
legacy exemptions. Keep these attributes in the source module so Cargo and
Bazel consume the same policy. The existing strict Cargo Clippy job enforces
it; ordinary Bazel compilation is not itself a Clippy run.

Prefer pattern matching and error propagation for ordinary optional state.
Unavoidable assembly invariants require an item-level `#[expect]` with a
specific reason and must not broaden into file/module allowances. Test builds
retain assertion and panic-injection helpers; this exemption must not disable
the normal production-library Clippy target in an all-targets invocation.

Verification must inject each prohibited construct into a temporary production
TUI function and observe its named Clippy error, then remove the probe and
validate the clean production and test targets. Root-crate lint migration is
tracked separately by #871. These gates do not prove absence of all panics,
including indexing, allocation failure, or dependency code.

## Follow-Up Quality Gates

These are proposed follow-ups, not installed gates:

- A monotonic baseline for presentation dependencies in state/projection modules;
  resolved baseline entries must disappear. Raw-color construction already has
  the QUALITY-05 Clippy boundary.
- A small width/height matrix for command/help/model/approval surfaces,
  including CJK, emoji, multiline paste, and empty results.
- Broader approval interleavings beyond the installed cancel/completion and
  stale-session guards.
- Wall-clock redraw/scroll baselines for long transcripts, building on the
  installed deterministic work-count guards.
- Physical-terminal and optional tmux resize acceptance outside required CI,
  complementing isolated PTY lifecycle tests.

These checks need concrete defect examples, owners, runtime budgets, and RED
evidence before becoming required jobs. Track the open work in [TODO](../todo.md).

## Source Journals

- [TUI interaction contracts and reference analysis](../journal/2026-09-17-tui-interaction-contracts.md)
- [Input ownership and draft preservation](../journal/2026-09-17-tui-input-ownership.md)
- [Permission controls and approval layout](../journal/2026-09-17-tui-permission-controls.md)
- [Terminal oracles and lint gates](../journal/2026-10-03-tui-quality-gates.md)
- [Production event-loop verification](../journal/2026-10-03-tui-event-loop-verification.md)
- [Scoped TUI panic lints](../journal/2026-10-04-tui-panic-lints.md)
