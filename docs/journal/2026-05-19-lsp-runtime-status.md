# LSP Runtime Status

## What changed

- Wired a per-workspace `LspManager` into runtime bootstrap instead of leaving
  it as an unused module.
- Registered the built-in `lsp_diagnostics` tool with the shared manager.
- Parsed `textDocument/publishDiagnostics` notifications into the diagnostics
  cache used by the tool and prompt injection.
- Shared the manager with the TUI and added a wide-sidebar LSP status section.

## Design notes

- The TUI reads a status snapshot from the shared manager rather than owning LSP
  state. This keeps rendering separate from language-server lifecycle.
- Sidebar status does not spawn language-server checks. Availability is cached
  only after a tool call needs to detect or start a server.
- `RARA_LSP=0`, `RARA_LSP=false`, and `RARA_LSP=off` disable the manager without
  removing the tool surface; the tool reports a structured error payload.

## Verification

- `cargo fmt`
- `cargo test lsp_manager -- --nocapture`
- `cargo test push_lsp_status -- --nocapture`

## Follow-up

- Replace the initial sleep-based LSP initialize flow with response-aware
  handshake handling.
- Add config-file LSP server overrides after the config schema exists.
