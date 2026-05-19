# Language Server Protocol (LSP) Integration

## Problem

RARA agents currently navigate and edit code using text-based tools only — `grep`,
`rg`, `glob`, string replacement. These have no semantic understanding:

- `rg "render"` matches comments, strings, unrelated functions
- Renaming a struct field requires manual find/replace across all files
- Type errors and borrow-checker failures are invisible until `cargo check` runs
- The agent wastes tokens and turns on grep/compile/edit cycles that an LSP
  could resolve instantly

OpenCode and Codex CLI both integrate LSP diagnostics as structured feedback
to the LLM. Claude Code runs inside LSP-capable editors but has no bridge to
the language server infrastructure.

RARA should expose LSP diagnostics and, eventually, semantic operations
(find references, rename, go-to-definition) as tools in the agent loop.

## Scope

- Auto-detection of `rust-analyzer` (already installed in Rust toolchains)
- `lsp_diagnostics` tool — returns current diagnostics for a file
- Diagnostics injected into System context on each turn (like warnings)
- Configuration surface for custom LSP servers
- Non-Goal: advanced LSP features like completion, hover, or symbol search (phase 2)

---

## Design

### 1. Architecture

```
Agent calls lsp_diagnostics("src/main.rs")
        │
        ▼
┌───────────────────┐
│   LspManager      │  ← singleton, per-workspace
│                   │
│  servers:         │
│   rust-analyzer   │  ← auto-detected, stdio transport
│   (future: gopls) │
│                   │
│  diagnostics:     │
│   file → diag[]   │  ← cached, updated on read
└───────────────────┘
        │
        ▼
  rust-analyzer (stdio)
  → publishDiagnostics notification
```

`LspManager` holds:
- A map of `language_id → LspServerHandle`
- Each handle wraps a `tokio::process::Child` + JSON-RPC reader/writer
- Diagnostics are cached per-file, invalidated on next `textDocument/didOpen` + `didChange`

### 2. Auto-Detection

RARA auto-detects Rust projects via `Cargo.toml` presence. If `rust-analyzer`
is on `$PATH`, it starts automatically on first `lsp_diagnostics` call.

Future: extend to other languages via `package.json`, `go.mod`, `deno.json`,
etc., following OpenCode's detection table.

### 3. Configuration

```jsonc
// ~/.rara/config.json
{
  "lsp": {
    "enabled": true,           // default: true
    "auto_install": false,     // default: false (manual opt-in)
    "servers": {
      "rust": {
        "command": ["rust-analyzer"],
        "extensions": [".rs"]
      },
      "custom": {
        "command": ["my-lsp", "--stdio"],
        "extensions": [".custom"]
      }
    }
  }
}
```

Per-project override via `AGENTS.md` or `.rara/lsp.json`.

### 4. Runtime Tool

| Tool | Signature | Description |
|------|-----------|-------------|
| `lsp_diagnostics` | `(file: Path)` → `Vec<Diagnostic>` | Current LSP diagnostics for a file |

The tool result keeps a JSON payload in the transcript so the TUI can render it
with a dedicated diagnostics cell instead of showing generic formatted JSON.

```rust
struct Diagnostic {
    file: PathBuf,
    range: Range,          // line:col start..end
    severity: Error | Warning | Info | Hint,
    message: String,       // e.g. "cannot find type `Foo` in this scope"
    code: Option<String>,  // e.g. "E0425"
    source: Option<String>,// e.g. "rustc"
}
```

### 5. System Context Injection

On each turn, RARA appends a `# LSP Diagnostics` block to the System context
if diagnostics exist for files in the current workspace:

```
# LSP Diagnostics
  src/local_model_server.rs:245:12 error[E0425]: cannot find type `BundledFile`
  src/tui/render.rs:89:5 warning: unused variable `width`
```

This is analogous to how `local embedding backend bootstrap reported:` warnings
work today. The agent sees diagnostics without needing to run `cargo check`.

### 6. Lifecycle

```
1. Agent opens/edits a .rs file
2. LspManager lazily starts rust-analyzer (first use)
3. LspManager sends textDocument/didOpen → textDocument/didChange → textDocument/didSave
4. rust-analyzer pushes publishDiagnostics asynchronously → LspManager caches
5. Agent calls lsp_diagnostics("src/foo.rs") → returns last-cached diagnostics (no sync wait)
6. Diagnostics cache persists until file modification or server push invalidates it
```

`lsp_diagnostics` reads cached diagnostics directly — it never blocks waiting
for a server notification. This avoids the common LSP client pitfall where
`publishDiagnostics` may be debounced or skipped when diagnostics haven't
changed. If the server supports LSP 3.17+ pull diagnostics
(`textDocument/diagnostic`), those can be used as a fallback.

---

---

## Design Decision: Built-in Tool, Not MCP

LSP is integrated as a **built-in tool** (like `bash`, `read_file`), not as
an MCP server.

| Dimension | Built-in Tool | MCP |
|-----------|-------------|-----|
| Lifecycle | RARA manages (lazy spawn, auto-recycle) | External process management required |
| Diagnostic injection | Auto-inject into System context, agent sees without calling | Agent must discover + call |
| Protocol overhead | JSON-RPC (LSP native) | JSON-RPC → MCP → LSP |
| Configuration | `~/.rara/config.json` directly | Through MCP config layer |
| Latency | Process already running, diagnostics cached | Every call goes through MCP round-trip |

MCP is for external services (browsers, databases, third-party APIs).
LSP is a local process under RARA's control, same as `rust-analyzer` via stdio.

OpenCode follows this exact pattern — LSP diagnostics are fed directly to
the LLM, not mediated through an MCP bridge.

---

## Implementation Plan

### Phase 1: Rust-Analyzer Bridge (~150 lines)

- `LspManager` struct with lazy `rust-analyzer` spawn
- Response-aware JSON-RPC handshake (`initialize`, then `initialized` after
  the matching response)
- `textDocument/didOpen`, `textDocument/didChange`, `textDocument/didSave`
- Parse `textDocument/publishDiagnostics` notifications
- `lsp_diagnostics` tool

### Phase 2: Context Injection (~40 lines)

- Append `# LSP Diagnostics` to System context when diagnostics exist
- Deduplicate and format diagnostic messages
- Clear on agent turn boundary

### Runtime status

The runtime owns a per-workspace `LspManager` and shares it with the
`lsp_diagnostics` tool and the TUI. The wide sidebar shows:

- whether LSP is initialized, disabled, detected, idle, running, or missing;
- detected server names for the current workspace markers;
- the current cached diagnostic count;
- the last startup or request error when one is available.

Sidebar rendering must not spawn language-server availability checks. The
status view only reports cached availability until a tool call needs to verify
and start a server.

`lsp_diagnostics` tool results render as a structured TUI cell with the target
file, diagnostic count, severity, source location, optional diagnostic code,
message preview, cached runtime count, and startup/request error when present.

### Phase 3: Configuration (~50 lines)

- Read `~/.rara/config.json` `lsp` section
- Per-project `.rara/lsp.json` override
- Environment variable `RARA_LSP` to disable

### Phase 4: Multi-Language (~60 lines)

- Detection table (file extension → LSP server)
- Generic LSP server spawn from config
- `go.mod` / `package.json` / `deno.json` detection

---

## Verification

- Open a Rust file with a type error → `lsp_diagnostics` returns the error
- Fix the error → `lsp_diagnostics` returns empty
- Disable via `RARA_LSP=0` → `lsp_diagnostics` returns "LSP disabled"
- `# LSP Diagnostics` block appears in System context with current errors
- Start RARA without `rust-analyzer` on PATH → graceful degradation ("LSP not available")

---

## Prior Art

- **OpenCode** (`opencode.ai/docs/en/lsp/`): 30+ built-in LSP servers, auto-detect
  via file extensions, `lsp: true` to enable all. Diagnostics fed to LLM.
- **Codex CLI** (issue #8745): Proposed LSP Manager with auto-install,
  `codex --lsp=auto`, `codex lsp status`. Built-in server mapping table.
- **Claude Code** (issue #24249): Requested bridge from IDE LSP to Claude tools.
  Not yet implemented.
