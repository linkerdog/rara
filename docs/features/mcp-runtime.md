# MCP Runtime

## Problem

RARA needs a Claude-style MCP integration surface that can be configured from
both user-level settings and project-level files without silently changing tool
availability. MCP servers affect tools, resources, prompts, approvals, context
budget, and future ACP/Wire adapters, so the integration must start with a
source-aware registry instead of ad hoc tool injection.

## Scope

This spec covers:

- user MCP configuration in `~/.rara/config.toml` under `[mcp_servers.*]`;
- project MCP configuration in `<workspace>/.mcp.json` under `mcpServers`;
- source and scope tracking for every configured server;
- hard failure when the same server name appears in multiple sources;
- future status, refresh, reconnect, resource, and Tool Search contracts.

## Non-Goals

- Starting MCP server processes in the first slice.
- OAuth login for MCP servers.
- Enterprise-managed MCP policies.
- Project approval UI for newly discovered `.mcp.json` servers.
- Letting MCP tools bypass RARA approval, sandbox, or transcript policy.

## Architecture

### Source Scopes

RARA tracks MCP definitions with explicit scope provenance:

- `user`: `~/.rara/config.toml`;
- `project`: `<workspace>/.mcp.json`;
- `local`: reserved for machine-local project settings;
- `enterprise`: reserved for managed policy;
- `builtin`: reserved for RARA-provided MCP surfaces.

Claude Code groups MCP servers by scope in its management UI. RARA should keep
the same source clarity, but it must not silently override conflicting server
names across `config.toml` and `.mcp.json`. A duplicate name is a configuration
error and startup/status refresh should fail with both source paths.

### Configuration Shapes

User config follows the Codex-style TOML shape:

```toml
[mcp_servers.docs]
command = "docs-server"
args = ["--stdio"]

[mcp_servers.remote]
url = "https://example.com/mcp"
bearer_token_env_var = "MCP_TOKEN"
```

Project config follows the Claude-compatible JSON shape:

```json
{
  "mcpServers": {
    "repo": {
      "command": "repo-mcp",
      "args": ["--root", "."]
    }
  }
}
```

Supported transports:

- stdio: `command`, optional `args`, `env`, and `cwd`;
- streamable HTTP: `url`, optional bearer-token environment variable and
  headers.

### Registry Boundary

`McpRegistry` is the stable configuration boundary. It owns the effective map of
server name to:

- transport config;
- enablement and required flags;
- startup/tool timeout hints;
- enabled/disabled tool filters;
- source scope and path.

Runtime connection managers, `/mcp`, `/status`, `/context`, ACP, and Wire should
read from the registry instead of reparsing files.

### Status Model

The runtime connection layer should later expose one structured status per MCP
server:

- `configured`;
- `connecting`;
- `connected`;
- `disconnected`;
- `refreshing`;
- `reconnecting`;
- `failed`;
- `disabled`.

The status event must include server name, scope, source path, transport kind,
last error, discovered tools count, discovered resources count, and whether the
server is required.

The first implemented status slice is read-only. It derives `configured` and
`disabled` from `McpRegistry`, preserves scope/source provenance, redacts URL
secrets, and reports load failures through `/mcp` without starting any server
process. Later connection-manager work should update the same status objects
instead of adding a second representation.

### Dynamic Refresh

MCP tool/resource/prompt lists can change after startup. RARA should support:

- manual refresh through `/mcp refresh`;
- automatic refresh when a server emits list-changed notifications;
- structured runtime events so TUI, ACP, Wire, and future appserver clients see
  the same refreshed state.

Refresh must preserve prompt-cache stability. Newly discovered tools should not
be appended directly to the core prompt unless the Tool Search policy chooses to
expose them.

### Auto Reconnect

Disconnected HTTP streams or crashed stdio servers should move through:

```text
connected -> disconnected -> reconnecting -> connected | failed
```

Reconnect should use bounded exponential backoff and should stop retrying for
non-retryable config errors. Users should also be able to trigger reconnect
manually.

### Resource References

MCP resources should be first-class context sources rather than copied into the
system prompt by default. A future resource reference should carry:

- server name;
- resource URI;
- title or display label;
- MIME type when known;
- token estimate;
- scope and source path.

`/context` should show selected and available MCP resources with the same
provenance rules used for files, memory, skills, and prompt sources.

Implementation checkpoint:

- RARA now has a typed `McpResourceReference` adapter that normalizes MCP
  resource references into `RetrievalCandidate` objects.
- `ContextAssembler` carries precomputed MCP resource candidates through the
  same retrieval and `MemorySelection` pipeline as memory, thread, vector, and
  file-search candidates.
- `/context` provider status includes an `mcp_resource` source entry with a
  reference count, and candidate provenance keeps server name, URI, MIME type,
  source path, and token estimate.
- Resource bodies are not loaded or injected in this slice. Until a content
  loader exists, MCP resource candidates remain referenced and visible but
  non-selectable, preserving prompt-prefix stability.

### Tool Search

RARA should not inject every MCP tool schema into every turn. Large MCP
installations make context unstable and consume budget. The target model is:

1. inject a small, stable MCP Tool Search entrypoint;
2. index discovered MCP tool names, descriptions, schemas, and server scopes;
3. let the model search for relevant tools when needed;
4. expose or call only the selected tool set for the turn.

This follows the same cache-prefix principle used by prompt sources and memory:
large dynamic surfaces should be searched or referenced, not eagerly appended.

## Contracts

- Loading user `config.toml` and project `.mcp.json` is deterministic.
- Missing config files produce an empty registry.
- Duplicate server names across sources fail loudly.
- Source scope and path are preserved for every server.
- Registry parsing is independent from TUI rendering and future connection
  startup.
- `/mcp` renders the registry-derived status grouped by scope and source path.
- `/mcp` must report parse/conflict failures instead of silently ignoring broken
  config.
- `/mcp` publishes the registry-derived status as a structured runtime event so
  ACP, Wire, and future appserver subscribers do not need to parse TUI text.
- `/mcp` publishes a structured `status_load_failed` event when registry loading
  fails so subscribers can drop stale status.
- MCP runtime-control requests use the `mcp` request family with
  `query_status`, `refresh { server_name? }`, and
  `reconnect { server_name }` variants. These request names are part of the
  external control-plane contract.
- MCP status snapshots must carry display-safe targets only. Stdio command
  targets and HTTP URLs are redacted before entering either TUI text or
  structured runtime events.
- MCP tools, resources, prompts, and status changes must later enter the
  runtime control plane as structured events.

## Validation Matrix

| Case | Expected result |
| --- | --- |
| only `~/.rara/config.toml` has MCP servers | registry contains user-scoped servers |
| only `.mcp.json` has MCP servers | registry contains project-scoped servers |
| both files define distinct server names | registry contains both with source paths |
| both files define the same server name | load fails with both source paths |
| neither file exists | empty registry |
| invalid TOML or JSON | load fails with parse path |
| `/mcp` with configured servers | grouped status includes scope, source path, transport, state, and tool filters |
| `/mcp` with no servers | status says no MCP servers are configured |
| `/mcp` with runtime subscribers | emits an `mcp.status_updated` runtime event with the same snapshot |
| `/mcp` load failure with runtime subscribers | emits an `mcp.status_load_failed` runtime event |
| MCP control request serde | locks `query_status`, `refresh`, and `reconnect` wire shapes |
| MCP server target contains secrets | status snapshot stores only redacted display text |

## Open Risks

- Project `.mcp.json` can start arbitrary stdio commands once connection startup
  exists. RARA should add project trust or per-server approval before spawning.
- HTTP MCP auth and OAuth state need a separate credential policy.
- Tool Search needs careful prompt design so tools remain discoverable without
  bloating the system prompt.
- Dynamic refresh and reconnect need bounded retry policy to avoid noisy loops.

## Source Journals

- `docs/journal/2026-05-05-mcp-config-registry.md`
- `docs/journal/2026-05-05-mcp-status-surface.md`
- `docs/journal/2026-05-05-mcp-runtime-events.md`
