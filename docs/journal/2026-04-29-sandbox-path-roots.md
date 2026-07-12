# 2026-04-29 Sandbox Command Path Roots

## Summary

- Aligned RARA sandbox command visibility with the Codex pattern of preserving a
  controlled execution environment while keeping filesystem access scoped.
- Linux bubblewrap now derives command install roots from `PATH` entries. For
  `bin` and `sbin` directories, the sandbox binds the installation prefix
  read-only instead of only binding the executable directory.
- macOS seatbelt profiles now grant read and executable-map access to the same
  `PATH` install roots.
- Runtime startup now captures a lightweight shell environment snapshot and
  shares the captured `PATH` with sandbox mount construction, bash commands, and
  PTY sessions.

## Notes

- Root-level `/bin` and `/sbin` remain bound as themselves so the Linux sandbox
  does not fall back to a read-only bind of `/`.
- The change avoids binding the whole home directory while still allowing
  common user-installed tool layouts such as `~/.cargo/bin`,
  `~/.local/bin`, and Homebrew-style prefixes to resolve symlinks and adjacent
  runtime files.
- The first snapshot slice captures `PATH` only. This mirrors the Codex and
  Claude direction of reusing user shell initialization while avoiding a full
  shell-state import before RARA has a richer environment policy.
- Command-root classification now receives the resolved home directory as an
  explicit internal input. This preserves the runtime environment behavior and
  lets path-root tests avoid mutating process-wide `HOME` while running in
  parallel.
