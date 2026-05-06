# Terminal Environment Detection

## Problem

RARA currently handles terminal differences through local checks such as SSH
session detection and targeted rendering fallbacks. This is enough for narrow
cases, but it does not provide a stable runtime identity for:

- TUI compatibility decisions;
- terminal-specific rendering workarounds;
- OTEL attributes and user-agent metadata;
- mux-aware behavior for tmux and Zellij;
- support debugging when output corruption depends on the host terminal.

Codex keeps this concern in a separate terminal-detection module. RARA should
follow the same boundary: detect terminal metadata once, expose structured
facts, and let TUI, telemetry, and diagnostics consume those facts.

## Goals

- Add a small terminal environment detection module with a stable
  `TerminalInfo` shape.
- Replace scattered environment checks with typed helpers where practical.
- Keep terminal detection independent from rendering code.
- Make the detected terminal metadata reusable by future OTEL exporters.
- Preserve existing SSH and Zellij behavior while making the detection source
  explicit.

## Non-Goals

- Do not redesign the TUI renderer in this slice.
- Do not add a full OTEL exporter as part of terminal detection.
- Do not run shell probes unless they are explicitly guarded and low impact.
- Do not make terminal detection a hard dependency for startup success.

## Detection Model

The initial model should expose:

```rust
pub struct TerminalInfo {
    pub name: TerminalName,
    pub term_program: Option<String>,
    pub version: Option<String>,
    pub term: Option<String>,
    pub multiplexer: Option<Multiplexer>,
    pub remote: Option<RemoteSession>,
}
```

`TerminalName` should cover at least:

- Apple Terminal;
- Ghostty;
- iTerm2;
- Warp;
- VS Code;
- WezTerm;
- kitty;
- Alacritty;
- Konsole;
- GNOME Terminal / VTE;
- Windows Terminal;
- dumb;
- unknown.

`Multiplexer` should cover:

- tmux;
- Zellij.

`RemoteSession` should cover SSH-style sessions currently detected by
`SSH_CONNECTION` and `SSH_TTY`.

## Detection Order

Detection should prefer explicit terminal identity over capability strings:

1. multiplexer markers such as `TMUX`, `TMUX_PANE`, `ZELLIJ`,
   `ZELLIJ_SESSION_NAME`, and `ZELLIJ_VERSION`;
2. `TERM_PROGRAM` plus `TERM_PROGRAM_VERSION`;
3. terminal-specific variables such as `WEZTERM_VERSION`,
   `ITERM_SESSION_ID`, `KITTY_WINDOW_ID`, `ALACRITTY_SOCKET`,
   `KONSOLE_VERSION`, `GNOME_TERMINAL_SCREEN`, `VTE_VERSION`, and
   `WT_SESSION`;
4. `TERM` fallback;
5. unknown.

When running under tmux, RARA may later add an optional guarded probe for
`tmux display-message` to identify the underlying client terminal. The first
implementation can skip the probe and still expose tmux as the multiplexer.

## Consumers

### TUI

The TUI should use terminal metadata for compatibility decisions:

- Zellij-specific history insertion strategy;
- SSH startup warnings and remote-session behavior;
- future terminal-specific workarounds for cursor movement, bracketed paste,
  scrollback clearing, and display sanitization.

### OTEL And Diagnostics

Future telemetry should attach sanitized terminal attributes:

- `terminal.name`;
- `terminal.term_program`;
- `terminal.version`;
- `terminal.term`;
- `terminal.multiplexer`;
- `terminal.remote`.

The same metadata should be visible in `/status` or `/context` once those
surfaces have a diagnostics section. Display text must be rendered from
structured metadata, not parsed back from logs.

## Implementation Plan

1. Add a `terminal_environment` module with `TerminalInfo`, `TerminalName`,
   `Multiplexer`, and `RemoteSession`.
2. Add injectable environment access for deterministic tests.
3. Implement detection from environment variables only.
4. Replace `tui::terminal_ui::is_ssh_session()` with
   `TerminalInfo::is_remote_session()` while preserving behavior.
5. Replace direct Zellij checks, if any, with `TerminalInfo::is_zellij()`.
6. Add focused tests for major terminal families, SSH, tmux, Zellij, and
   `TERM=dumb`.
7. Later, expose sanitized user-agent / OTEL attribute helpers.

## Open Questions

- Whether tmux client probing should run at startup, be lazy, or be disabled by
  default.
- Whether terminal metadata should be persisted in session records for later
  debugging.
- Whether `/status` should show terminal metadata by default or hide it behind
  a diagnostics tab.

## Source Journals

- [2026-05-06-terminal-environment-detection](../journal/2026-05-06-terminal-environment-detection.md)
