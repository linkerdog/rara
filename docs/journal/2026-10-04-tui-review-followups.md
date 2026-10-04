# TUI Review Follow-Ups

## Scope And Plan

Address the remaining review observations on #949, #951, and #953 in three
stages: narrow lint expectations; extend production-loop and session shutdown
coverage; remove checked-unwrapping from goal admission and document crash
resume limits. Each stage keeps its focused validation boundary before the
final TUI suite and strict workspace Clippy run. No public API, persisted
format, dependency, or Bazel configuration changes are planned.

The references are local Codex `f959e7fc9832dfa0ebfb6542ab1bbf829638ac24`
(`tui/src/tui.rs` exit restoration and `ext/goal/src/runtime.rs` idle admission)
and Claude Code `4b9d30f7953273e567a18eb819f4eddd45fcc877`
(`src/ink/ink.tsx` suspend/resume and unmount ownership). Adapt their explicit
terminal ownership and admission boundaries to the existing event loop and
goal store; do not add another scheduler or goal loop.

The stages have separate exit conditions:

1. Preserve palette values and test diagnostics while moving exceptions to
   their owning items. Exit when strict Clippy passes and unrelated helper
   mutations are rejected. This avoids changing the palette abstraction.
2. Keep the production select loop and terminal ownership unchanged. Add a
   complete isolated session fixture and route the existing suspend fixture
   through the production input adapter. Exit when quit, cancellation, handoff,
   and two suspend/resume cycles pass, with mutations proving the new guards.
   PTYs still do not prove physical-terminal or multiplexer acceptance.
3. Preserve goal lifecycle, permissions, and persistence contracts. Acquire
   agent ownership explicitly and return it on failed/refused admission. Exit
   when failure and mode-waiting tests pass. A new persisted crash counter is
   outside this review fix; document the existing retry limitation instead.

## Decisions

- Color and print exceptions belong to concrete owners, not entire modules.
- Component cancellation tests complement actual select-loop coverage.
- Plan mode retains automatic goal requests until eligible; explicit user
  requests retain their existing separate admission semantics.
- Abrupt process crashes do not currently create a durable continuation
  deferral. Document that limitation rather than claim a crash-loop guard.

## Implementation

- Theme RGB constants and color parser/test functions now carry individual
  expectations. Clipboard cleanup and PTY protocol functions have their own
  print expectations; surrounding helpers remain protected.
- Actual-loop tests cover double Ctrl-C, `/quit`, and cancellation of a running
  query. The fake port captures the translated cancel command, which the test
  delivers through the real processor/select branch. The token and task-drop
  signals prove that first cancellation waits and confirmed quit aborts.
- A full `run_tui` child uses a real PTY, isolated working/state directories,
  and an injected non-network backend. Its parent sends `/quit` after the first
  real frame and verifies final newline/cursor placement before mode resets,
  successful return, and kernel termios restoration. Direct invocation of the
  ignored child requires its isolation marker.
- The existing two-cycle job-control child now constructs the same
  `TerminalEventSource` as the production session, covering reader take/replace,
  resumed input, resize, and late shell mode repair. Fields stay private; only
  the adapter and trait are shared within the TUI module.
- Goal admission takes a ready agent before claiming the ticket and restores
  it on either refusal or persistence failure. Plan mode retains automatic
  requests until eligible, while explicit requests keep their existing path.

## Validation

Focused loop checks pass 21 cases plus the ignored child exercised by its PTY
parent. The goal cases include declined admission, an injected budget-status
write failure, and a Plan-to-Execute transition without losing the request.

Both temporary lint mutations were rejected: an RGB constructor in an ordinary
`ThemeToken` method and `println!` in the neighboring PTY output helper.
The expectation scopes and original source were restored after the probes.

Three independent production mutations failed at the intended assertions:

| Mutation | Regression evidence |
| --- | --- |
| Continue the loop instead of breaking after confirmed quit | The keyboard-quit test remains pending instead of returning success |
| Drop the join handle instead of aborting outstanding work | The paused-time task-drop deadline expires after confirmed quit |
| Omit the session's final inline viewport handoff | The full PTY session lacks a newline between its last frame and mode restoration |

Each mutation was restored before the next probe and before final validation.
No compile failure is counted as behavior evidence. Final Cargo and default
Bazel TUI suites each pass 985 cases, with four isolated child fixtures exercised
by their parent tests. Strict workspace/all-target Clippy, formatting, and diff
checks pass. A test-only byte-slicing Clippy warning was corrected before the
final suite runs; no snapshots changed.

Missing external Bazel caches were restored with targeted module, registry,
bootstrap, and toolchain fetches. Active registry repositories came from the
lockfile's generated HTTP archive specs, avoiding obsolete cached repository
names and unrelated Git downloads. No configuration or lockfile changes were
needed. The final default target took 99 seconds, including 22 seconds of tests;
the existing gold-linker deprecation warning remains unrelated.

```bash
cargo test --lib tui::
cargo clippy --workspace --all-targets -- -D warnings
bazel test //:rara_unit_tests --test_arg=tui::
cargo fmt --all -- --check
git diff --check
```

## Follow-Ups

Remote CI/review/merge and physical macOS/tmux acceptance remain separate gates.
