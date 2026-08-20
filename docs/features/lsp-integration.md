# Language Server Protocol Integration

## Problem

RARA needs semantic diagnostics without turning a language-server startup into
a synchronous tool timeout. The previous bridge waited on one response channel,
dropped mismatched responses, discarded server stderr, repeatedly sent
`didOpen`, slept the calling thread, and reported only `running: false` after a
five-second initialization failure.

## Scope

- Per-workspace, lazy language-server lifecycle for Rust, Go, and TypeScript.
- Asynchronous JSON-RPC initialization and response routing.
- Cached push diagnostics with explicit freshness.
- Structured startup/request failures and pure cached status snapshots.
- `lsp_diagnostics` tool and System-context diagnostic injection.

## Non-Goals

- Auto-installing language servers.
- Completion, hover, rename, references, or symbol search.
- Provider-specific or editor-mediated LSP bridges.
- Blocking until a new `publishDiagnostics` notification arrives.
- Treating an empty diagnostic set as a startup failure.

## Architecture

`LspManager` is session-scoped and owns one slot per detected server kind. Each
slot has a serialized startup gate and a cached state machine:

```text
NotStarted -> Starting -> Ready
                    |-> Unavailable
                    `-> Failed
```

Only one startup future may run for a slot. Concurrent callers wait for that
same startup gate and then reuse the resulting connection. A ready connection
contains:

- one async stdin writer;
- a response router keyed by JSON-RPC request ID;
- an async stdout reader for responses and notifications;
- a shared protocol writer that answers required server-to-client control
  requests while initialization is in flight;
- a bounded stderr tail;
- a child supervisor that observes exit and cancellation;
- document versions used to reject stale diagnostics.

The manager uses filesystem markers such as `Cargo.toml`, `go.mod`, and
`package.json` for detection. Status rendering reads cached slot state only; it
must not execute `--version`, spawn a server, or block on I/O.

## Contracts

### Startup and retry

- The first matching `lsp_diagnostics` call lazily starts the server.
- Initialization uses a valid directory `file://` URI and waits asynchronously
  for the matching response ID.
- The default initialization deadline is 45 seconds; tests and embedded callers
  may configure a shorter deadline.
- Concurrent calls cause at most one spawn attempt.
- Startup observes the initialize response, child exit, stderr tail, timeout,
  and cancellation concurrently.
- Retry uses bounded attempts and backoff. Missing binaries are
  `Unavailable` and are not hot-looped.

### Server control requests

The protocol loop must continue serving server-to-client requests while a
client request is pending. It returns valid responses for:

- `window/workDoneProgress/create`;
- `workspace/configuration`;
- `workspace/workspaceFolders`;
- `client/registerCapability` and `client/unregisterCapability`;
- `workspace/diagnostic/refresh`.

Unsupported server requests receive JSON-RPC `-32601` rather than being
silently dropped. RARA advertises only the workspace configuration, workspace
folder, work-done progress, synchronization, and push-diagnostic capabilities
implemented by this bridge.

### Structured failures

Failures expose a stable kind and human-readable message. Required kinds are:

- `disabled`
- `unsupported_file`
- `binary_missing`
- `spawn_failed`
- `initialize_timeout`
- `protocol_error`
- `server_exited`
- `file_read_failed`

When available, the failure includes the server name, process exit/signal, and
bounded stderr tail. The legacy top-level `error` string remains in the tool
payload for compatibility; the typed failure is canonical.

### Document synchronization and diagnostics

- The first synchronization sends `textDocument/didOpen` with version 1.
- Later synchronizations send `textDocument/didChange` with monotonically
  increasing versions; `didOpen` is never repeated for an already-open file.
- Calls do not sleep for diagnostics. After synchronization they immediately
  return the cache with `freshness = "current"`, `"cached"`, or `"pending"`.
- `pending` means no diagnostic publication exists for the current document;
  it is not an error.
- Publications older than the current document version are discarded.
- An empty, current publication replaces previous diagnostics with an empty
  set.

### Status and context

- `status_snapshot()` is pure over cached state and cheap enough for TUI render
  paths.
- Each server reports its phase, detected/checked/available/running flags, and
  last typed failure when present.
- The workspace snapshot retains `last_error` for compatibility and also
  exposes `last_failure`.
- Cached diagnostics may be injected into System context. Injection never
  starts or waits for a server.

### Tool result

Successful or pending calls return:

```json
{
  "file": "src/main.rs",
  "diagnostics": [],
  "freshness": "pending",
  "status": {}
}
```

Failed calls additionally return `failure` and the legacy `error` string. An
empty diagnostic array with no failure is a valid result.

## Validation Matrix

| Contract | Focused check |
|---|---|
| Matching response routing | Interleave response IDs and verify each waiter receives its own response |
| Server control request | Issue `workspace/configuration` before initialize completes and verify a shaped response |
| Delayed initialize | Fake server responds within the configured timeout |
| Timeout | Fake server stays alive without responding and returns `initialize_timeout` |
| Early exit | Fake server writes stderr, exits, and returns `server_exited` with the tail |
| Concurrent startup | Two diagnostics calls share one spawn attempt |
| Cancelled startup | Abort an in-flight initialize and verify status changes to `Failed` and the next call can retry |
| Document lifecycle | First call emits `didOpen`; next call emits versioned `didChange` |
| Stale diagnostics | Older publication is dropped; current empty publication clears cache |
| Pure status | Repeated snapshots do not execute or spawn a command |
| Graceful absence | Missing binary reports `binary_missing` and `Unavailable` |

## Operational Notes

The child supervisor owns process waiting and cancels the server when the last
runtime handle is dropped. Stderr capture is bounded so a broken server cannot
grow memory without limit. Diagnostics remain a cache: compilation commands are
still the authoritative repository validation gate.

## Open Risks

- Capability-specific server requests beyond the control methods above receive
  a JSON-RPC method-not-found response and need explicit handlers before RARA
  advertises the corresponding capability.
- Pull diagnostics (`textDocument/diagnostic`) remain a future fallback for
  servers that do not publish diagnostics.
- Custom server configuration and per-project overrides remain future work.

## Source Journals

- [2026-08-20-agent-tool-reliability](../journal/2026-08-20-agent-tool-reliability.md)
