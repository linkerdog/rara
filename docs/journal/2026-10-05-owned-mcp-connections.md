# Owned MCP Stdio Connections

## Summary

The MCP client now owns a callable stdio connection, bounds incoming frames,
validates all catalogue pages before admission and explicitly retires its direct child. Existing tool
discovery reuses the same path instead of reading one page and dropping a live
service. No app-server capability is enabled by this component alone.

## Background And Decisions

Inspection of local reference checkouts informed the ownership boundary:
Codex `f959e7fc` resolves MCP configuration per thread and queues refresh through
the thread actor; Claude Code `4b9d30f7` applies managed policy to dynamic server
injection and reconnects a specific source. Session registration must preserve
those boundaries rather than reuse a process-global connection catalogue.

The selected `rmcp` single-request API returns input-required responses to the
owner. The high-level helper can automatically resubmit requests and would
cross the outer operation journal's retry authority. Transport errors and
timeouts therefore remain explicitly uncertain.

The connection retains its own child handle. SDK service-close completion alone
does not prove child retirement; SDK timeout also consumes its cleanup handle.
Shutdown waits or kills and reaps the child, and interrupted cleanup cannot
become a successful receipt on a second attempt. Drop remains best effort.

## Validation

Focused Unix tests use isolated real Python stdio children. They cover complete
paged discovery and calls, explicit environment ownership, catalogue rejection,
no retransmission after a lost result or input-required response, failed
handshake cleanup, ordinary shutdown, forced termination and interrupted cleanup.
Fragmented messages remain readable; an oversized unterminated frame closes
the connection without first accumulating the rest of the response.

Commands:

```bash
cargo test -p rara-mcp-client --lib
cargo clippy -p rara-mcp-client --all-targets -- -D warnings
cargo fmt --all --check
bazel test --test_output=errors //crates/rara-mcp-client:rara_mcp_client_tests
```

## Follow-Ups

The [runtime contract](../features/mcp-runtime.md) and
[active backlog](../todo.md#app-server-stdio) retain the session-registry gate:
explicit opt-in, namespaced atomic admission, session identity,
actual native tool invocation and exact source retirement. Frozen host profiles
must not be widened, and nested agents must not inherit parent credentials.
The outer supervisor still owns descendant containment and durable operations.
