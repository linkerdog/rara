# Keyboard Enhancement Ownership

## Background And Plan

Issue #988 identified that the composer binds Shift+Enter but never enables
terminal keyboard disambiguation. The change stays in terminal mode ownership;
the existing crossterm decoder and key routing remain authoritative.

References inspected before implementation:

- Codex `f959e7fc9832dfa0ebfb6542ab1bbf829638ac24`,
  `codex-rs/tui/src/tui/keyboard_modes.rs`: push enhanced keyboard flags during
  ownership and restore the stack during temporary handoff.
- Claude Code `4b9d30f7953273e567a18eb819f4eddd45fcc877`,
  `src/ink/components/App.tsx`, `termio/csi.ts`, and `terminal.ts`: pair keyboard
  mode setup with raw input ownership and use the minimal disambiguation flag.
- [Kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/):
  progressive flags, stack push/pop, and legacy encoding.
- Local crossterm 0.29.0 `terminal/sys/unix.rs`: the support query waits up to
  two seconds for flags, then performs an unbounded device-attributes read;
  repeated polling errors also retry without an overall deadline.

Implementation:

1. Push only disambiguation on Unix TTY acquisition. Track the single owned
   stack entry across panic-hook cleanup, guard restoration, and suspend.
2. Extend lifecycle PTY coverage and exercise actual CSI-u bytes through
   crossterm and the existing event translator.
3. Validate key routing, repeated job-control cycles, formatting, and Clippy.

## Decisions

- Use a non-querying progressive enable, as supported by Codex's setup pattern.
  Unknown ANSI commands fall back to legacy terminal input. This avoids adding
  a potentially unbounded startup/resume query or a second input reader/parser.
  Claude's terminal-name allowlist is not copied: environment names can be
  absent or misleading over SSH. Native Windows retains the existing path.
- Request flag 1 only. Report-event-types, alternate-key reporting, terminal
  name heuristics, modifyOtherKeys, and new bindings are outside this fix.
- Arm keyboard restoration after the full push command is accepted and before
  the fallible flush. A failed partial command must not pop the parent's entry;
  a flush error may occur after the push reached the terminal. Consume the marker
  before popping, including on panic; a later Drop cannot pop another entry.
- Do not use Codex's stronger full-reset exit sequence: this TUI must preserve
  keyboard modes inherited from its caller. Ctrl+J remains the newline fallback
  when the terminal cannot distinguish Shift+Enter.

## Validation

- Before implementation, the PTY regression failed on the missing push command.
  The new job-control assertion separately reproduced a push before the shell
  stopped-job prompt, demonstrating the pre-existing suspend race.
- `cargo test --locked --lib tui::`: 1046 passed, 4 isolated child entry points
  ignored by the outer runner and driven by parent tests. Snapshots unchanged.
- The terminal fixture covers 14 lifecycle scenarios, including failed partial
  writes, flush errors, panic-hook/Drop idempotence, and 15 enhanced/legacy input
  sequences decoded by crossterm and routed by the production translator.
- `cargo test --offline --lib tui::job_control::tests -- --nocapture`: normal two-cycle shell
  suspension and ignored-stop error handling. The first stop exceeds the nominal
  runnable polling budget; both cycles retain input, resize, and shell state.
- The foreground suspend/resume regression passed 20 additional consecutive runs
  of the compiled test binary, covering 40 real shell stop/resume cycles.
- `cargo clippy --locked --workspace --all-targets -- -D warnings`,
  `cargo fmt --all -- --check`, and `git diff --check` are clean.

Native Windows and physical terminal emulators were not manually exercised.
The local default Bazel external-cache failure remains unchanged; CI owns that
validation. No Bazel configuration or cache changes are part of this fix.

## Suspend Boundary Correction

The new per-cycle stack assertions exposed a pre-existing race: `killpg` may
return before another thread handles SIGTSTP. A real shell PTY captured a fresh
keyboard push after cleanup but before the shell's stopped-job prompt. The
existing termios assertions did not detect these leaked terminal commands.

Suspension now registers a short-lived SIGCONT flag before stopping and
gates all mode reacquisition on it. Use the already resolved `signal-hook`
crate's safe registration API and unregister the action when leaving suspend.
Bound runnable polling so ignored/blocked stop signals cannot hang the UI;
on timeout, retain the restored terminal and surface the error. Count polling
attempts rather than wall-clock elapsed time, which includes suspension.
This adapts Claude Code's SIGCONT-driven resume pattern and corrects the
process-group-return assumption also present in the inspected Codex helper.
The delivery model is documented in
[signal(7)](https://man7.org/linux/man-pages/man7/signal.7.html): a process-directed
signal may be delivered to any unblocked thread.
