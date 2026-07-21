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

Git sources are now accepted by the same install command. Sources beginning
with `https://`, `ssh://`, `git://`, `file://`, or `git@` are cloned with
`git clone --depth 1` into a temporary checkout, validated with the same
`rara-plugins` loader path, copied into the workspace plugin directory, and
then removed from the temporary checkout location. The installed plugin name
still comes from `.claude-plugin/plugin.json`, not from the repository URL.

Verification:

- `cargo test app_cli -- --nocapture`
- `cargo test plugin_cli -- --nocapture`
