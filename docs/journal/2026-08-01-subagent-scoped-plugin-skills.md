# Subagent Scoped Plugin Skills

## What changed

Subagent definitions can now declare `pluginSkills` as an explicit allowlist.
The app-server/runtime layer copies those named plugin-scoped skills into a
child-owned `SkillManager` and registers a scoped `skill` tool. The child has
no plugin roots, cannot reload the manager, and cannot invoke workspace or
global skills through this capability.

The default remains deny-by-default. Unknown names and non-plugin skills fail
child construction. Tool allowlists and disallow lists also control whether
the `skill` tool can be exposed. MCP servers, MCP tools, and plugin memory
access remain denied.

## Why

The parent runtime owns plugin discovery and credentials. Sharing the parent
registry with a subagent would make prompt-level policy misleading and allow
implicit authority inheritance. A copied in-memory registry gives the child a
stable snapshot while preserving the runtime boundary.

## Trade-offs

The snapshot becomes stale when the parent reloads plugins; rebuilding the
child is required to observe changes. This is intentional because child
reload would otherwise become an unscoped discovery path. MCP needs a
separate scoped executor because server transport and credential ownership
cannot be represented by a skill snapshot.

## Verification

- `cargo test --bin rara tools::skill:: --no-fail-fast`
- `cargo test --bin rara subagent_plugin_capability_policy --no-fail-fast`
- `cargo test --bin rara scoped_plugin_skill_tool --no-fail-fast`
- `cargo fmt --all -- --check`

## Follow-up

Implement an app-server-owned scoped MCP executor without granting children
parent credentials or unrestricted server discovery.
