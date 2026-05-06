# Terminal Environment Detection Plan

## Context

Codex has a dedicated terminal-detection boundary that identifies terminal
family, version, `TERM`, and active multiplexers. RARA currently uses narrower
checks, especially SSH detection inside the TUI layer.

## Decision

Record terminal detection as a first-class RARA runtime concern. The first
implementation should be small and environment-only, with typed metadata shared
by TUI compatibility code, future OTEL attributes, and diagnostics surfaces.

## Follow-Up

- Implement the `terminal_environment` module.
- Replace ad hoc SSH and Zellij checks with typed helpers.
- Add tests for common terminal families and mux/remote markers.
- Decide whether tmux client probing should be lazy or disabled by default.
