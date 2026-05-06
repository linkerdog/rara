# Terminal Environment Detection

## Context

Codex has a dedicated terminal-detection boundary that identifies terminal
family, version, `TERM`, remote sessions, and multiplexers. RARA previously used
ad hoc SSH checks inside the TUI layer.

## Implementation Checkpoint

- Added `crates/terminal-detection` with typed terminal, multiplexer, and
  remote-session metadata.
- Preserved SSH behavior through `rara_terminal_detection::is_remote_session()`.
- Connected Zellij detection to committed-history insertion mode in the TUI
  terminal layer.
- Added focused unit tests for terminal families, mux markers, SSH markers,
  `TERM=dumb`, and user-agent sanitization.

## Follow-Up

- Add diagnostics display after `/status` has a terminal/runtime details tab.
- Add OTEL attributes from the same `TerminalInfo` data model.
- Revisit optional tmux client probing only if there is a concrete rendering or
  attribution need.
