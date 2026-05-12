# 2026-05-12 Claude Plugin Runtime

## What We Built

Created the `rara-plugins` crate (`crates/rara-plugins/`) with three modules:
loader, exec, types. Supports 6 lifecycle events, parses plugin.json and
hooks/hooks.json, executes command hooks via spawn + stdin JSON.

## Why

Claude Code plugin ecosystem compatibility.

## Trade-offs

No JS engine. Command hooks only. No MCP launching yet.
No matcher evaluation in first slice.

## Remaining

- Wire into HookRuntime
- Plugin install CLI
- MCP launcher
- Prompt hook integration
