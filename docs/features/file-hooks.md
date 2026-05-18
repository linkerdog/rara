# File-Based Hooks (Prompt Extensions)

## Motivation

RARA needs to load Claude Code-style `.claude/hooks/*.md` files and inject
their content into the system prompt so the model can follow hook-specific
instructions during the appropriate lifecycle phases.

Unlike plugin hooks (which execute shell commands), file-based hooks are
**prompt extensions** — their content is loaded into the system prompt and the
model is expected to follow the instructions.

## Design

### Directory Layout

```
<workspace>/.claude/hooks/
├── pre-tool-use.md       # prompt before tool calls
├── post-tool-use.md      # prompt after tool results
├── session-start.md      # prompt at session beginning
├── session-end.md        # prompt at session end
├── stop.md               # prompt when agent stops
├── user-prompt-submit.md # prompt when user submits
├── notification.md       # prompt for desktop notifications
```

### Discovery

At startup (`runtime_context.rs`), `HookRegistry::discover_repo_hooks()` scans
`.claude/hooks/` under the workspace root. Each `.md` file is parsed into a
`HookDefinition` with:

- `id` — filename stem (e.g. `pre-tool-use`)
- `phase` — `HookLifecycle` variant derived from the filename
- `body` — raw markdown content

Discovery is best-effort: broken files are skipped with a warning.

### Context Injection

During `ContextAssembler::assemble()`, all discovered hook content is collected
via `PromptRuntimeConfig::hooks_prompt()` and injected into the system prompt
under a `## Hooks` section, together with the `## Memory Summary` section.

Hook content is injected **before** the memory summary to maximise
attention on hook instructions.

### Lifecycle Mapping

| File | HookLifecycle |
|---|---|
| `pre-tool-use.md` | `PreToolUse` |
| `post-tool-use.md` | `PostToolUse` |
| `session-start.md` | `SessionStart` |
| `session-end.md` | `Stop` |
| `stop.md` | `Stop` |
| `user-prompt-submit.md` | `UserPromptSubmit` |
| `notification.md` | `Notification` |

### Limitations (Current Phase)

- Hooks are injected unconditionally — no per-phase conditional injection.
  All hook content is always present in every turn.
- Hook content is not truncated; very large hooks may inflate the system
  prompt.
- There is no `--hooks-enabled` flag; hooks are always loaded at startup.
- Hook execution (command-type hooks) is not implemented — this spec covers
  prompt injection only.

## Integration Points

- `hooks.rs` — `HookRegistry`, `HookDefinition`, `discover_repo_hooks()`, `discover_from_dir()`
- `runtime_context.rs` — calls `discover_repo_hooks()` at startup, populates `PromptRuntimeConfig.hook_prompt_text`
- `crates/instructions/src/prompt.rs` — `PromptRuntimeConfig::hooks_prompt()` method
- `context/assembler.rs` — injects `## Hooks` section into effective prompt

## Verification

- `cargo test --bin rara` — all tests pass
- Manually: create `.claude/hooks/pre-tool-use.md` with content, start RARA,
  confirm hooks appear in the system prompt
