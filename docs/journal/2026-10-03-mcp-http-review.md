# MCP HTTP Discovery Review

## Summary

PR #888 now applies the registry's loopback proxy policy, resolves header names
case-insensitively, bounds complete paginated tool discovery, and redacts nested
probe diagnostics. Cloud REST endpoints derive from the normalized MCP base,
including custom deployment prefixes and a trailing `/mcp/`.

## References And Plan

The reference review used Codex `ea2046f36d5ee12d39c8e168fc3e5129301afa2b`
(`codex-rs/rmcp-client/src/rmcp_client.rs`) and Claude Code
`4b9d30f7953273e567a18eb819f4eddd45fcc877`
(`src/services/mcp/client.ts`). Codex explicitly constructs the HTTP client and
passes it to the transport; Claude Code separates connection and request
deadlines. This implementation adapts those boundaries to the existing
registry and ten-second startup probe policy.

The implementation order was: define the discovery contracts, reproduce URL
and header precedence regressions, repair the transport and logging boundaries,
then verify them against isolated local HTTP fixtures.

## Key Decisions

- `HttpProxyPolicy` carries the registry's routing decision into the thin client.
  `reqwest::ClientBuilder::no_proxy` is applied before transport construction.
  This supersedes the original journal's `from_config` choice and adds a direct
  dependency on the same locked reqwest version used by rmcp. Host matching
  remains in the configuration crate.
- Header keys are normalized before applying bearer, environment, and static
  sources in that order. Invalid headers still fail through the HTTP parser.
- Initialization and listing have separate ten-second deadlines. Both stdio
  and HTTP use rmcp's `list_all_tools`; all pages share one listing budget, and
  a later failure cannot publish an incomplete list.
- HTTP connection context omits the endpoint. The runtime warning boundary
  formats and redacts the entire error chain using the existing persistence
  redactor, retaining diagnostic causes without printing URL credentials.
- REST and MCP derivation share the normalized endpoint base, preserving local
  mode behavior and cloud deployment prefixes.

## Validation

- The new endpoint and mixed-case header tests failed on the original code:
  `/mcp/remote-api` was produced, and four headers survived instead of two.
- `cargo test --offline --locked -p rara-config --lib`: 64 passed.
- `cargo test --offline --locked -p rara-mcp-client --lib`: 9 passed, including
  real HTTP initialization, headers on every request, pagination, partial-list
  rejection, connection and aggregate listing deadlines, and proxy routing.
  The local socket fixtures require permission to bind loopback ports.
- Proxy environment tests use subprocess-local variables, with both direct and
  proxied positive controls. No process-global environment mutation or external
  service credentials are involved.
- `cargo test --offline --locked -p rara --lib mcp_tool_cache`: 7 passed,
  covering header precedence, missing variables, nested diagnostic redaction,
  and the existing refresh invalidation contract.
- `cargo clippy --offline --locked -p rara-mcp-client --all-targets --no-deps
  -- -D warnings`: passed. Panic-based fixture assertions are allowed only in
  the test module; production lint policy is unchanged.
- `cargo fmt --all` and default `bazel mod deps` completed. The generated Bazel
  lock now includes the HTTP transport graph; no Bazel configuration or BUILD
  files were edited.
- `cargo clippy --offline --locked --all-targets --no-deps -- -D warnings`:
  passed for the application targets.
- `bazel test //crates/rara-mcp-client:rara_mcp_client_tests`: passed after
  `bazel fetch --force //crates/rara-mcp-client:rara_mcp_client_tests` restored
  incomplete local external-repository caches. Earlier failures occurred during
  package loading, before compiling the changed code.

## Follow-Ups

Tool invocation remains tracked in `docs/todo.md`. Runtime-owned automatic
discovery for headless sessions and further cloud connector metadata remain
separate work; this review does not change those lifecycle boundaries.
