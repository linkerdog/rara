# MCP Streamable-HTTP Tool Index

## Summary

`rara-mcp-client` can now connect to streamable-HTTP MCP servers, the MCP tool
index covers those servers in addition to stdio servers, and the built-in
Nowledge Mem Cloud plugin points at the endpoint the Nowledge Mem connectors
register.

## Background

`McpServerTransport::StreamableHttp` was parsed into the registry and rendered in
`/mcp` status, but nothing connected to it: the tool cache skipped every
non-stdio server, `rmcp` was built with `transport-child-process` only, and
`env_http_headers` had producers but no consumer.

Cloud mode was unreachable for a second reason. The derived endpoint was
`https://cloud.nowledge.co/remote-api/mcp/`, which the hosted server answers with
`HTTP 404`. The connectors Nowledge Mem registers for other hosts use
`https://cloud.nowledge.co/mcp`.

## Scope

- `crates/rara-mcp-client`: `list_http_tools` plus shared tool record mapping.
- `src/mcp_tool_cache.rs`: streamable-HTTP branch in
  `populate_from_registry_owned` and the header resolution boundary.
- `crates/config/src/model.rs`: cloud MCP endpoint derivation.
- Docs: `docs/features/mcp-runtime.md`,
  `docs/features/claude-plugin-runtime.md`, `docs/todo.md`.

## Key Decisions

- `rmcp` gains the `transport-streamable-http-client-reqwest` and `reqwest`
  features. The `reqwest` feature selects rustls, which is also reqwest 0.13's
  default TLS path, so HTTPS works without native-tls.
- The transport builds its own reqwest client through
  `StreamableHttpClientTransport::from_config`, so `rara-mcp-client` does not
  depend on reqwest directly and cannot drift from the rmcp client type.
- All resolved headers are sent as `custom_headers`. `bearer_token_env_var`
  becomes a literal `Authorization: Bearer <token>` header instead of rmcp's
  `auth_header` field, because rmcp 3.4.0 documents the `Bearer` prefix handling
  of `auth_header` ambiguously.
- Header precedence is `bearer_token_env_var` < `env_http_headers` <
  `http_headers`, so an explicit static value always wins over an
  environment-derived value.
- A missing environment variable is reported with `log::warn!` and skipped. It
  fails neither that server nor the index build, and it stays visible in the TUI.
- The tool listing failure path now uses `log::warn!` instead of `eprintln!`, so
  connect failures surface in the TUI alongside the conversation.
- Cloud MCP endpoints derive as `<base>/mcp`. `api_url()` still returns
  `<base>/remote-api`, because that REST base is unchanged and was not shown to
  be wrong; only the MCP path was.
- Tool invocation stays out of scope. The index is discovery-only for both
  transports.

## Validation

- `cargo fmt --all`
- `cargo check -p rara-mcp-client --all-targets` — exit 0
- `cargo test -p rara-mcp-client --all-targets` — 3 passed
- `cargo test -p rara --lib mcp_tool_cache` — 4 passed
- `cargo test -p rara-config` — 63 passed
- `cargo test -p rara --lib plugin_middleware::tests` — 23 passed
- `cargo test -p rara --lib status_display` — 8 passed
- `cargo clippy -p rara-mcp-client --all-targets -- -D warnings` — exit 0
- A temporary probe binary called
  `list_http_tools("https://cloud.nowledge.co/mcp", ...)` and listed 40 Nowledge
  Mem tools, including `memory_search`, `memory_add`, and `read_context_bundle`.
  The probe was deleted after the run.
- Re-materialized `~/.rara/builtin-plugins/nowledge-mem/.mcp.json` now contains
  `"url": "https://cloud.nowledge.co/mcp"`.

## Follow-Ups

- MCP tool invocation is tracked in `docs/todo.md` under MCP Runtime.
- The tool index is populated from the TUI startup path only, so headless
  `exec`/`print` sessions do not index MCP tools yet.
- Nowledge Mem connectors also send `X-Nmem-Tool-Set`,
  `X-Nowledge-Tool-Schema-Profile`, and env-backed `X-Nmem-Agent-Id` /
  `X-Nmem-Host-Agent-Id`. RARA does not set them yet.
