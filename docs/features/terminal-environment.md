# Terminal Environment Detection

## Problem

RARA has terminal-specific behavior, but environment checks are currently
scattered across TUI code. That makes it harder to reason about rendering
workarounds, remote-session behavior, diagnostics, and future OTEL attributes.

Codex keeps terminal detection as a separate boundary. RARA follows the same
shape: detect terminal metadata once, expose structured facts, and let TUI,
telemetry, and diagnostics consume those facts.

## Goals

- Add structured terminal metadata through `TerminalInfo`.
- Keep detection independent from rendering code.
- Preserve existing SSH behavior through a typed remote-session helper.
- Route Zellij-specific history insertion through terminal metadata.
- Make the metadata reusable for future `/status`, `/context`, and OTEL work.

## Runtime Contract

Terminal detection exposes:

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

The first implementation detects only environment variables. It does not shell
out to tmux or fail startup when metadata is missing.

## TUI Relationship

Terminal detection does not render UI directly. It selects compatibility policy
for the terminal layer:

- `event_loop` owns the draw loop and calls `flush_committed_history`.
- `flush_committed_history` turns committed transcript entries into lines.
- terminal metadata selects `InsertHistoryMode`.
- `insert_history_lines_with_mode` performs the actual terminal escape strategy.

The important rendering branch is history insertion above the active viewport:

- standard terminals use scroll regions and Reverse Index;
- Zellij uses the fallback insertion strategy because it mishandles those escape
  sequences.

SSH detection also remains a TUI policy input for auth-mode defaults and browser
login warnings, but now comes from `TerminalInfo::is_remote_session()`.

## Detection Order

1. multiplexer markers: `TMUX`, `TMUX_PANE`, `ZELLIJ`,
   `ZELLIJ_SESSION_NAME`, `ZELLIJ_VERSION`;
2. `TERM_PROGRAM` plus `TERM_PROGRAM_VERSION`;
3. terminal-specific variables such as `WEZTERM_VERSION`, `ITERM_SESSION_ID`,
   `KITTY_WINDOW_ID`, `ALACRITTY_SOCKET`, `KONSOLE_VERSION`,
   `GNOME_TERMINAL_SCREEN`, `VTE_VERSION`, and `WT_SESSION`;
4. `TERM` fallback;
5. unknown.

## Follow-Up

- Surface terminal metadata in diagnostics once `/status` has a suitable tab.
- Add OTEL attributes from the same `TerminalInfo` shape.
- Decide whether tmux client probing should be lazy, disabled by default, or
  never added.

## Source Journals

- [2026-05-06-terminal-environment-detection](../journal/2026-05-06-terminal-environment-detection.md)
