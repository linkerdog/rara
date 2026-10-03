# Interrupt, Quit, And Unix Job Control

## Summary

The second #925 checkpoint adds bounded Ctrl-C/Ctrl-D quit confirmation and
Unix Ctrl-Z suspend/resume on top of the inline viewport work. Contracts live
in INPUT-02/03 and RUN-02/07.

## Reference And Plan

Inspected Codex `chatwidget/interaction.rs`, `bottom_pane/mod.rs`, `tui.rs`, and
`tui/job_control.rs` at `ea2046f36d5ee12d39c8e168fc3e5129301afa2b`, plus
Claude Code `hooks/useExitOnCtrlCD.ts` and `ink/components/App.tsx` at
`4b9d30f7953273e567a18eb819f4eddd45fcc877`.

Both implementations give an active input surface ownership before global
exit handling and restore terminal modes around suspension. Adapt Codex's
one-second same-key confirmation and foreground-process-group SIGTSTP. Retain
the existing busy Ctrl-C cancellation/draft contract. Use the existing inline
reservation after resume instead of introducing another cursor-probe path.

Implementation sequence: define the keyboard and job-control contracts; expose
the current routing failures with focused dispatch tests; add presentation
state and input routing; connect mode/viewport ownership to real Unix job
control; validate the combined behavior before publishing.

## Implementation

- A small state object tracks the armed shortcut and monotonic confirmation
  window. The existing maintenance tick expires its footer hint. Other input
  disarms it; terminal repeat metadata cannot confirm exit.
- Ctrl-C closes overlays first. Palette dismissal discards pending paste before
  flushing could change input ownership. In the main composer, first Ctrl-C
  preserves clear/cancel behavior and the second confirms exit. Busy cancellation
  still crosses the existing typed runtime port once.
- Ctrl-D confirms exit only with empty input and no overlay. Editable surfaces
  retain delete-forward behavior, and read-only surfaces do not insert text.
- Unix Ctrl-Z drops the event reader, finishes the inline viewport, restores
  modes, and signals the foreground job's process group using the safe `nix`
  API. Foreground resume reacquires modes and a fresh event reader. The next
  frame reserves new rows and repaints at the current terminal size.
- A real interactive-shell regression exposed the shell's post-SIGCONT termios
  restoration racing with mode acquisition. Codex's one-shot raw-mode repair
  was insufficient under repeated local validation. After suspension, the
  existing maintenance tick compares native termios against its raw projection
  and clears/reapplies crossterm's cached raw state only on drift. This keeps
  recovery independent of shell scheduling and avoids adding a fixed delay.
  Enable the existing `nix` dependency's `term` feature for safe termios access.
- Terminal owner polling remains scoped across suspension, preserving panic
  cleanup and keeping background-task panics separate from owner failures.

## Validation

The initial regression tests failed on unchanged production behavior: Ctrl-C
left Help open, quit hints were absent, and Ctrl-Z inserted `z` into the draft.
Focused tests also cover busy cancellation before confirmation, mixed shortcuts,
expiry, overlay/paste ownership, and reported repeats. A subprocess fixture uses
a separate PTY and Bash job-control shell to verify two actual stop/foreground
resume cycles, kernel termios, mode bytes, input restart, and resizing.
Each resume also injects a late cooked-mode restoration while crossterm still
reports raw mode. The fixture requires a single key without Enter to become
readable again, then verifies the native modes before permitting the next stop.
The fixture uses separate interactive `fg` commands: Bash abandons an active
compound command after a foreground job stops again, so a `-c` loop cannot
model repeated interactive resumes.

- `cargo test --locked --lib tui:: -- --nocapture`: 901 passed; two ignored
  subprocess fixtures are exercised by their parent PTY tests.
- The foreground suspend/resume PTY regression passed 20 consecutive runs
  (40 stop/resume cycles), including injected late cooked-mode restoration.
- `cargo clippy --locked --all-targets --no-deps -- -D warnings`: passed.
- `bazel test //:rara_unit_tests --test_arg=tui::`: passed with the same 901
  tests and two subprocess fixtures. Generated dependency metadata includes
  the `nix` termios feature; no Bazel configuration or BUILD edits were needed.
- `cargo fmt --check` and `git diff --check`: passed.

## Follow-Ups

Keep #925 open for bounded interactive terminal acceptance, including tmux and
macOS. The automated job-control fixture does not establish that matrix.
External stop signals, background `bg` resume, and non-Unix job control are
outside this keyboard-driven contract. Runtime-owned goal cancellation/resume
policy remains the separate #931 work.
