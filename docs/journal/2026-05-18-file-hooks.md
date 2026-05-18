# 2026-05-18 file-hooks context injection

## What was built

- `docs/features/file-hooks.md` — spec for file-based prompt-extension hooks
- `PromptRuntimeConfig::hooks_prompt()` — joins all discovered hook bodies for context injection
- `ContextAssembler::assemble()` — injects `## Hooks` section alongside `## Memory Summary`
- `runtime_context.rs` — calls `hooks::HookRegistry::discover_repo_hooks()` at startup using cwd

## Why

File-based hooks (`.claude/hooks/*.md`) are Claude Code-pattern prompt extensions.
They tell the model how to behave during lifecycle phases.  Previously the discovery
code existed (`hooks.rs`) but was never wired — hooks were loaded into memory but
never injected into the system prompt.

## Trade-offs

- All hooks are injected into every turn (no per-phase conditional injection).
  This is simpler but may dilute the effective context for phases where a
  hook is irrelevant.
- Hook content is not truncated.  Very large hooks must be manually kept small.
- Uses `std::env::current_dir()` instead of a stored workspace root — hooks are
  discovered relative to the cwd at startup time, matching Claude Code behavior.

## What remains

- Per-phase conditional injection (only show PreToolUse hooks before tool calls)
- Truncation policy for large hooks
- `--hooks-enabled` flag
- Command-type hook execution (`./executable` with JSON stdin/stdout)
