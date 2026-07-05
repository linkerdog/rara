# RARA Exec Headless

## Summary

Added the first Codex-style headless automation surface for RARA:
`rara exec`.

The command reuses the normal agent loop without TUI chrome and supports:

- prompt argument, stdin, and `-` stdin-only prompt handling;
- `--cwd` / `-C` task workspace selection;
- `--json` JSONL trajectory events;
- `--run-id` and `--task-id` metadata for external harnesses;
- `--output-last-message` for harnesses that need a final answer file.

## Background

Harbor's Terminal-Bench tutorial drives agents through an installed-agent
adapter. RARA needs a stable non-interactive surface that such an adapter can
invoke before adding Harbor-specific packaging and ATIF conversion.

Codex's `codex exec` shape is the closest reference: a generic headless
automation surface first, benchmark wrappers second.

## Key Decisions

- Keep benchmark logic outside the core agent runtime.
- Make `rara exec --json` the reusable automation boundary.
- Emit a RARA-owned JSONL event schema instead of exposing internal runtime
  events directly.
- Defer Harbor installed-agent packaging and ATIF conversion to the next slice.

## Validation

- `cargo test exec_consumer`
- `cargo test app_cli::tests::clap_parses_exec_command_for_headless_harnesses`
- `cargo check --locked --workspace --all-targets`
- `cargo clippy --locked --workspace --all-targets -- -D warnings`
- `cargo run --locked -- --provider mock exec --json --run-id smoke --task-id smoke-task "Say hello"`

## Follow-Ups

- Add the Harbor installed-agent adapter that invokes `rara exec --json`.
- Convert RARA JSONL events into ATIF-compatible trajectory artifacts.
- Run Harbor's Terminal-Bench tutorial command with the RARA adapter and record
  the exact invocation.
