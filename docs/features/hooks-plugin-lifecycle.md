# Hooks And Plugin Lifecycle

## Problem

RARA has several hook surfaces: file-based prompt hooks, runtime-control hook
declarations, plugin hook registration, command hook output, and memory-query
callbacks. These surfaces need one lifecycle contract so hook output can become
observable model context without bypassing safety or retrieval boundaries.

## Scope

- Lifecycle names and timing for hook/plugin integration.
- Hook output flow into model context and `/context` observability.
- MemoryQuery hook dispatch from the `search_memory` tool.
- Plugin hook registration and execution boundaries.
- Claude Code-compatible project `Stop` command hooks that can gate completion.

## Non-Goals

- JavaScript, HTTP, or agent-backed hook handlers.
- Unbounded hook output injection.
- Hook execution that bypasses sandbox, approval, timeout, or provenance rules.
- Replacing file-based prompt hooks.
- Reading benchmark verifier files, hidden tests, or external evaluation metadata.

## Architecture

Hook declarations normalize into RARA-owned lifecycle phases:

- `SessionStart`
- `UserPromptSubmit`
- `PreToolUse`
- `PostToolUse`
- `PostMemoryWrite`
- `MemoryQuery`
- `PreCompact`
- `PostCompact`
- `Stop`

The in-process `HookRuntime` subscribes to `RuntimeEventBus` events and invokes
registered callbacks for matching lifecycle phases. Lifecycle phases that do not
map directly to an `AgentEvent`, such as `MemoryQuery`, must use explicit
runtime methods instead of overloading unrelated events.

Command hook output is buffered in `HookRuntime`. Before each model turn, the
agent drains the buffer, injects each output as a system message, and mirrors the
same output as volatile `hook_output` retrieval candidates. These candidates are
observable in `/context` but marked non-selectable because the output has already
entered the model context directly.

## Contracts

- Hook output must be bounded by the hook runtime and drained once.
- Drained hook output is next-turn context. It must not be reselected by memory
  retrieval after direct injection.

Project settings may declare a Claude Code-compatible command Stop hook in
`.claude/settings.json`:

```json
{
  "hooks": {
    "Stop": [{
      "hooks": [{
        "type": "command",
        "command": "./scripts/verify-completion.sh",
        "timeout": 30
      }]
    }]
  }
}
```

RARA currently supports the `hooks.Stop` command-handler subset. The command
runs in the workspace and receives JSON on stdin containing `session_id`,
`cwd`, `hook_event_name`, `stop_hook_active`, and `last_assistant_message`.
It may only rely on files and commands available in that workspace.
- `MemoryQuery` hooks fire before `search_memory` runs and receive the raw query
  text through a lifecycle callback.
- Plugin hooks are registered during runtime rebuild/startup before the runtime
  snapshot is refreshed.
- Plugin hook failures must be visible through warnings or hook status; silent
  failure is not allowed.
- Hook output candidates use `source_type=hook_output`, `scope=turn`, and a
  session-scoped source reference.
- A Stop command returning exit code `2` blocks completion and its stderr is
  supplied to the next model turn as a system message. Exit code `0` may instead
  print JSON `{ "decision": "block", "reason": "..." }`, or
  `hookSpecificOutput.additionalContext`, to continue with feedback.
- Other non-zero exits and hook timeouts are visible as status warnings but do
  not block completion, matching Claude Code's exit-code semantics.
- RARA allows at most eight consecutive Stop-hook continuations before it
  completes the turn, preventing an unsatisfiable hook from consuming the
  entire agent budget.

## Validation Matrix

- Hook runtime tests cover `MemoryQuery` dispatch and lifecycle isolation.
- Agent context tests cover hook-output candidate shape and non-selectability.
- Tooling checks cover that `search_memory` is registered with a MemoryQuery
  callback.
- Stop-hook tests cover Claude settings discovery, exit-code blocking, JSON
  blocking output, and model-visible continuation feedback.
- Workspace validation should run `cargo check --locked --workspace
  --all-targets` and `cargo clippy --locked --workspace --all-targets --
  -D warnings`.

## Open Risks

- Command hook execution remains constrained by the current sandbox policy.
- Hook output is injected directly before the next model turn; later work may
  route prompt-type hooks through the retrieval budget if a stronger selection
  policy is needed.
- Only the `Stop` command-handler subset is compatible today; matcher, HTTP,
  prompt, and agent hook types need separate contracts before they are enabled.

## Source Journals

- `docs/journal/2026-07-04-hooks-context-lifecycle.md`
- `docs/journal/2026-07-12-claude-stop-hooks.md`
