# MCP Main Integration

## Scope

Bring the reviewed streamable-HTTP discovery branch forward to main. Source,
configuration, and documentation merge without changing the discovery contract;
only the generated Bazel dependency lock requires reconciliation.

## Preserved Contracts

Retain the [review checkpoint](2026-10-03-mcp-http-review.md): normalized cloud
MCP/REST endpoints, case-insensitive header priority, registry-owned loopback
proxy bypass, separate initialization and aggregate listing deadlines, complete
pagination, and redacted error chains. Main's portable contracts, browser
effects/clocks, workspace resolution, and TUI changes remain intact.

Regenerate the crate index from the combined manifests and Cargo lock using the
default Bazel configuration. Audit generated dependency changes rather than
choosing one branch's stale index. No transport behavior, public API, or external
service credential policy is added by this integration.

## Validation

- Cargo checks pass: 64 configuration tests, 9 MCP client tests with real local
  HTTP/proxy fixtures, 37 application MCP tests, 23 plugin tests, and 8 status
  display tests. The five review fixes retain their existing implementation and
  fixture coverage.
- Strict workspace/all-target Clippy, formatting, whitespace checks, 42 native
  core/agent tests, and browser all-target compilation pass.
- Default Bazel index generation succeeds after targeted recovery of missing
  generator packages. The generated changes add `sse-stream`, rmcp HTTP/SSE
  features, direct dependency aliases, and input hashes only; existing package
  versions remain unchanged.
- The local default MCP Bazel test stops before compilation because the existing
  external cache lacks `platforms//host` package files. Exact-head remote default
  Bazel build/test remains required; no Bazel configuration was changed.
- The PR records the published full revision, fresh downstream Git acceptance,
  and remote CI. The older branch's green checks are not integration evidence.

## Remaining Work

MCP invocation and automatic headless discovery remain separate work under the
existing [runtime contract](../features/mcp-runtime.md) and [backlog](../todo.md).
This checkpoint does not claim a live cloud credential probe.
