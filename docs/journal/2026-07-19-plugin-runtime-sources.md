# Plugin Runtime Sources

## Summary

TUI runtime hook registration now discovers Claude plugins from both the user
plugin directory and the project plugin directory through the ordered
source-aware discovery API.

## Background

The previous plugin runtime path registered hooks from only the workspace
plugin directory. After source-aware discovery landed, runtime startup needed to
use that contract instead of rebuilding a one-directory scan path.

## Scope

- Added middleware source construction for:
  - `~/.rara/plugins` as the user plugin source;
  - `<workspace>/.rara/plugins` as the project plugin source;
  - explicit plugin directories as the final CLI source tier for future callers.
- Updated TUI runtime rebuild to call plugin hook registration with RARA home
  and the workspace root.
- Preserved the current behavior of not creating project-local `.rara` during
  plugin discovery. Missing plugin directories simply discover zero plugins.
- Added focused middleware tests for source ordering and project-over-user
  de-duplication.

## Key Decisions

- Source precedence is `user -> project -> explicit CLI`, with later sources
  overriding earlier sources by plugin name.
- This slice only wires the TUI runtime rebuild path. Headless, ACP, and Wire
  startup parity remains separate work.
- This slice does not add matcher evaluation, blocking hook behavior,
  `SessionEnd` dispatch, or plugin extension registry ingestion.

## Validation

```bash
cargo test plugin_middleware -- --nocapture
cargo test -p rara-plugins -- --nocapture
cargo test plugin_ -- --nocapture
cargo check --locked --workspace --all-targets
cargo fmt --check
git diff --check
```
