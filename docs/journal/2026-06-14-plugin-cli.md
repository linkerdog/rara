# 2026-06-14 - Plugin CLI Commands

RARA now exposes workspace plugin management through:

- `rara plugin install <SOURCE> [--force]`
- `rara plugin list`
- `rara plugin remove <NAME>`

The commands operate on the current workspace's `.rara/plugins` directory,
matching the runtime registration path. `install` accepts a local Claude Code
plugin directory, validates that it loads through `rara-plugins`, then copies it
under the plugin name from `.claude-plugin/plugin.json`. Existing plugins require
`--force` to replace. `remove` validates names before deleting so callers cannot
escape the workspace plugin directory.

Verification:

- `cargo test app_cli -- --nocapture`

