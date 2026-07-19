# Plugin Source Discovery

## Summary

Claude plugin discovery now preserves source metadata and exposes an ordered
multi-source discovery API. This closes the discovery contract slice without
expanding hook execution semantics or changing plugin trust boundaries.

## Background

RARA already had a `rara-plugins` crate, local-directory plugin CLI commands,
and runtime hook registration for workspace plugins. The remaining integration
gap was not basic plugin loading, but source-aware discovery that can later
combine user, project, and explicit CLI plugin directories without duplicating
loader logic.

## Scope

- Added `PluginSource` metadata for user, project, CLI, and generic directory
  origins.
- Added `PluginDiscoverySource` plus ordered multi-source discovery with
  name-based de-duplication.
- Preserved the existing single-directory `discover_plugins` and `load_plugin`
  APIs as compatibility wrappers.
- Marked workspace plugin CLI and runtime hook registration as project-sourced
  plugin discovery.

## Key Decisions

- Later entries in `discover_plugins_from_sources` override earlier entries
  with the same plugin name. Callers own their precedence policy by ordering
  the source list explicitly.
- This slice does not add global user plugin scanning to runtime startup yet.
  The runtime still registers workspace plugins while the loader now supports
  the broader source contract.
- Hook lifecycle behavior, matcher evaluation, blocking hook results, git
  source installs, and `.mcp.json`/commands/skills/agents integration remain
  separate follow-ups.

## Validation

```bash
cargo test -p rara-plugins -- --nocapture
cargo test plugin_ -- --nocapture
cargo check --locked --workspace --all-targets
cargo fmt --check
git diff --check
```
