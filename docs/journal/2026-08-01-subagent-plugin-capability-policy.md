# Subagent Plugin Capability Policy

## What Changed

RARA now attaches a runtime-owned `SubagentPluginCapabilityPolicy` to every
child subagent prompt as a separate final section. The policy denies plugin
skill execution, MCP server and tool execution, and plugin memory reads and
writes by default. It also records the maximum child delegation depth as one.

## Why

Plugin discovery and MCP registration belong to the app/runtime assembly layer.
A child session must not acquire the parent session's plugin registry,
credentials, or external execution authority merely because it was spawned.
The policy makes this boundary explicit while leaving the existing subagent
tool filtering unchanged.

## Trade-offs

The policy follows the existing RARA section pipeline, keeping the stable
prompt prefix and making the capability section observable as
`subagent_capability_policy`. It is descriptive and establishes the runtime
contract; prompt text is not enforcement. Direct skill and MCP execution remain
disabled until scoped runtime executors and explicit allowlists are
implemented. Memory read and write capabilities are modeled separately so a
future read-only Nowledge Mem integration does not implicitly grant persistence
authority.

## Verification

- `cargo test subagent_plugin_capability_policy_defaults_to_deny -- --nocapture`
- `cargo fmt --check`
- `cargo clippy --package rara --tests -- -D warnings`
- `git diff --check`

## Follow-up

Implement a runtime-owned scoped plugin skill executor first, then a read-only
MCP executor. Add memory write capability only after lifecycle, provenance, and
failure propagation are covered.
