# Terminal Status Diagnostics

Terminal detection already lived behind the `rara-terminal-detection` crate, but `/status` still
reported only the TUI focus flag. That left terminal compatibility decisions hard to inspect and
made future OTEL fields likely to drift from the runtime behavior.

This checkpoint adds a `TerminalDiagnosticsView` on `TuiApp` and routes status rendering through it:

- detected terminal name and sanitized user-agent token;
- optional `TERM` and `TERM_PROGRAM`;
- multiplexer and remote-session classification;
- selected history insertion mode;
- current TUI focus and terminal width.

The `/status` config tab and status runtime text now read the same structured view. Future OTEL
attributes should reuse this view instead of reading process environment variables directly.
