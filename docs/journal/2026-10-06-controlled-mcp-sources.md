# Controlled MCP Session Sources

## Summary

The native session actor owns explicit MCP source registration, atomic tool
catalogue admission, scoped invocation and source retirement. The app-server
adds the `mcp_source.register`, `mcp_source.unregister` and `mcp_source.query`
methods over its existing bounded receipts and ordered event stream.

## Boundaries

- Builders require explicit opt-in. Exact host tool registries stay unchanged
  by default; frozen profiles and session-stable schemas reject dynamic sources.
- Launches use an absolute executable, explicit arguments and environment, and
  the session workspace. Ambient environment is cleared. The outer controller
  remains responsible for authorizing the executable and credentials.
- Bounded, complete catalogues are admitted together. Stable hashed namespaces
  avoid provider name restrictions; existing tools and other sources cannot be
  overwritten. Runtime source events contain safe IDs and names only.
- Calls require the owning session, workspace, turn and call context. Retired
  handles cannot execute again, and plan/review modes cannot invoke controlled
  tools based merely on untrusted read-only annotations.
- Removal fences new calls before waiting for direct-child retirement. Source
  uncertainty blocks new turns and successful cleanup receipts. Connection and
  operation retry authority remain with the outer supervisor.
- The legacy MCP status manager is unchanged and cannot acknowledge these source
  controls. Nested agents receive no controlled connection or credentials.

## Validation

Focused fixtures cover real source children, foreign contexts, collisions,
retained handles after removal, busy admission, frozen host profiles, operational
uncertainty, native provider tool invocation and correlated stdio receipts.
The executable smoke adds a local provider tool call through an actual app-server
and controlled MCP child, then verifies retirement before semantic shutdown.

The 7 source/native tests, 10 stdio tests and dedicated read-only-mode test pass.
The actual binary passes all six smoke scenarios: normal shutdown, a controlled
source call/removal, stdin EOF, malformed input, truncation and output loss.
Root all-target Clippy passes with warnings denied, and formatting is clean.
The native tool result follows the existing compact transcript representation;
tests inspect typed ownership/error fields and the independent child request log
instead of assuming the presentation content is raw MCP JSON.

Relevant commands:

```bash
cargo test --locked --lib runtime_session::mcp_sources::tests
cargo test --locked --lib app_server_stdio::tests
cargo test --locked --lib controlled_mcp_tools_require_execute_mode
cargo clippy --locked --all-targets --no-deps -- -D warnings
cargo build --locked --bin rara
python3 scripts/app_server_smoke.py target/debug/rara
cargo fmt --all --check
```

These deterministic fixtures do not establish configured-provider acceptance or
the outer proxy's durable operation guarantees. Those remain explicit integration
gates in [the active backlog](../todo.md#app-server-stdio).
