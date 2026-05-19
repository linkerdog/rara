# Hook Subsystem

## Overview

RARA has two hook systems: **file-based prompt extensions** and **registry-based
command hooks**. Both use the shared `HookLifecycle` enum for lifecycle phases.

## File-Based Hooks (`.claude/hooks/*.md`)

Prompt extensions loaded at startup from the workspace root. Content is injected
into the system prompt every turn under `## Hooks`.

### Discovery

`hooks.rs::HookRegistry::discover_repo_hooks(cwd)` scans `.claude/hooks/` for
`.md` files. Each file is parsed into a `HookDefinition` with:

- `id` — filename stem (e.g. `pre-tool-use`)
- `phase` — `HookLifecycle` derived from filename
- `body` — raw markdown content

### Lifecycle Mapping

| File | HookLifecycle |
|---|---|
| `pre-tool-use.md` | `PreToolUse` |
| `post-tool-use.md` | `PostToolUse` |
| `session-start.md` | `SessionStart` |
| `stop.md` | `Stop` |
| `user-prompt-submit.md` | `UserPromptSubmit` |

### Context Injection

In `context/assembler.rs`, hook content is injected alongside the memory
summary. Currently all hooks are injected unconditionally (no per-phase
filtering).

## Command Hooks (executable scripts)

### Runtime

`hook_runtime.rs::HookRuntime` subscribes to `RuntimeEventBus` and dispatches
`AgentEvent` stream to registered callbacks.

### Execution Model

- `make_command_hook(script_path, cwd, timeout_secs)` → `HookCallback`
- Spawns `run_command_hook()` in a background thread (non-blocking)
- Stdin receives JSON: `{"event": "ToolUse(...)"}`
- Timeout kills child process via PID
- Results logged to stderr

### Startup

`hook_runtime.rs::HookRuntime::start()` is idempotent (`AtomicBool`). Called
once at app bootstrap from `builder.rs`.

## Control-Plane Hooks

`hook_registry.rs::HookRegistry` manages control-plane hook declarations.
Currently shows a disclaimer that hooks are recorded but not yet executed.

## HookLifecycle Variants

- `PreToolUse` — before a tool is called
- `PostToolUse` — after a tool returns
- `SessionStart` — at session initialization
- `Stop` — when the agent stops

## Files

| File | Purpose |
|---|---|
| `hook_runtime.rs` | Event-bus subscriber, command hook executor |
| `hook_registry.rs` | Control-plane hook declarations |
| `hooks.rs` | File-system hook discovery |
| `runtime_control.rs` | `HookLifecycle` enum + `HookEvent` |

## Known Gaps

- No per-phase conditional injection (all hooks in every turn)
- `PreToolUse` doesn't modify tool input (stdout discarded)
- Hook output not injected into model context
- Plugin hook registration not wired into runtime startup
- No session_id in command hook JSON input
