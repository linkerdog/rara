# Subagent Enhancement — Claude Code-Compatible Agent Definitions

## Summary

Extend RARA's subagent system (`src/tools/agent.rs`) to support
Claude Code-compatible agent definitions loaded from `.rara/agents/`
with tool permission scoping, progress tracking, and task resume.

This makes RARA's subagent behavior consistent with Claude Code's
agent tool semantics while reusing the existing `spawn_agent` / `team_create`
infrastructure.

## Design

### Agent Definition Format

Compatible with Claude Code's `.claude/agents/*.md` frontmatter. RARA's
canonical project-local location is `.rara/agents/*.md`; `.claude/agents/*.md`
is a legacy compatibility import path.
The YAML frontmatter block defines the agent:

```markdown
---
name: code-reviewer
description: Reviews code for correctness, style, and security issues.
tools: [Read, Grep, Glob, Bash]
disallowedTools: [Write, Edit]
model: inherit
permissionMode: acceptEdits
maxTurns: 20
---

You are a code reviewer. When reviewing code, check for:
1. Correctness bugs
2. Style violations
3. Security issues
4. Performance problems

Report findings with file:line references.
```

RARA reads the frontmatter block and uses the markdown body as the default
system prompt for the subagent.

### `AgentDefinition` Type

```rust
pub struct AgentDefinition {
    pub name: String,
    pub description: String,
    /// Human-readable description of when to use this agent.
    pub when_to_use: Option<String>,
    /// Allowed tools. Empty = all tools allowed.
    pub tools: Vec<String>,
    /// Disallowed tools. Takes precedence over `tools`.
    pub disallowed_tools: Vec<String>,
    /// Model override. "inherit" = use parent model. If the specified model
    /// is not available on the configured provider, fall back to the default
    /// model for the current session.
    pub model: Option<String>,
    /// Permission mode override.
    pub permission_mode: Option<String>,
    /// Maximum agentic turns before force-stop.
    pub max_turns: Option<u32>,
    /// Whether to run as a background task.
    pub background: bool,
    /// Source: built-in, user, project, or plugin.
    pub source: AgentDefinitionSource,
    /// System prompt for the subagent.
    pub system_prompt: Option<String>,
}
```

### Loading And Runtime Cache

`AgentDefinitionCache::load()` scans:
1. Built-in agents (General, Explore, Plan) — hardcoded in RARA.
2. `~/.claude/agents/*.md` files in the user home.
3. `~/.rara/agents/*.md` files in the user home.
4. `.claude/agents/*.md` files in the workspace root.
5. `.rara/agents/*.md` files in the workspace root.

Resolves conflicts by loading lower-precedence roots first. Workspace agents
override user agents, and `.rara/agents` overrides `.claude/agents` at the same
scope. Built-in agents are always available as "general", "explore", "plan"
when no custom definition with the requested name is loaded.

The cache is constructed with the runtime and shared by the `spawn_agent`
tool and `/status` extension summary. Running `spawn_agent` must resolve
against the cached registry instead of scanning the filesystem on every call.
After editing `.rara/agents` or `.claude/agents`, the existing runtime keeps
its previous snapshot until the runtime is rebuilt. RARA intentionally does
not expose a dedicated `/reload-agents` command; this follows Claude Code's
pattern of refreshing agent definitions through existing runtime/plugin
refresh boundaries rather than adding an agent-only slash command.

The `/status` extension summary only lists repo-local agent definition records.
Definitions with `hidden: true` are omitted from listing/status surfaces while
remaining valid for direct `spawn_agent` resolution. Visible definitions include
their frontmatter `description` in the status line when present.

`permissionMode` controls the spawned subagent's local execution policy. Values
are parsed ASCII case-insensitively:

- `default` and omitted values keep the subagent's normal execution mode.
- `acceptEdits`, `accept-edits`, and `accept_edits` keep execute mode but
  require bash approval for mutable shell commands.
- `auto` keeps execute mode with the normal auto policy.
- `plan`, `readOnly`, `read-only`, and `read_only` force plan mode and a
  read-only tool manager.
- `bypassPermissions`, `bypass-permissions`, `bypass_permissions`,
  `fullAccess`, `full-access`, and `full_access` enable full-access approval
  bypass unless `planModeRequired` is also set, in which case plan mode takes
  precedence.

Invalid `permissionMode` values fail the `spawn_agent` request before creating
a subagent.

`tokenBudget` is an optional positive token budget for a spawned subagent. RARA
counts model input and output tokens reported by the provider. Cache hit and
miss counters remain visible in telemetry but are not added to the budget total.
When a budgeted subagent reaches or exceeds its budget, RARA stops before
starting another model turn, returns `status: "budget_limited"`, and persists
the budget on the parent spawn-agent edge. Invalid non-positive or oversized
values fail the `spawn_agent` request before creating a subagent.

### Subagent Execution Changes

**Before** (current):
```rust
fn call(&self, input: Value, ctx: ToolCallContext) -> Result<Value> {
    // Always uses General kind, full tool access.
}
```

**After**:
```rust
fn call(&self, input: Value, ctx: ToolCallContext) -> Result<Value> {
    let agent_name = input["agent_type"].as_str().unwrap_or("general");
    let definition = load_agent_definition(agent_name)?;
    let filtered_tools = apply_tool_filters(definition);
    // spawn with filtered tools and definition.system_prompt
    // Resolve model: if definition.model is set and available, use it;
    // otherwise fall back to the session's default model.
}
```

### Tool Permission Scoping

When a subagent is spawned with a definition that has `tools` or
`disallowedTools`, the `ToolManager` for the subagent is filtered:

```rust
fn filtered_tool_manager(
    definition: &AgentDefinition,
    base_manager: &ToolManager,
) -> ToolManager {
    let mut manager = base_manager.clone();
    for disallowed in &definition.disallowed_tools {
        manager.disable(disallowed);
    }
    if !definition.tools.is_empty() {
        // Whitelist mode: disable everything, then enable whitelisted.
        manager.disable_all();
        for allowed in definition.tools.iter().filter_map(|t| resolve_tool_name(t)) {
            manager.enable(allowed);
        }
    }
    manager
}
```

### Progress Tracking

Add to `SpawnAgent`:

```rust
pub struct SubagentProgress {
    pub tool_use_count: u32,
    pub total_tokens: u64,
    pub last_activity: Option<String>,
}
```

Updated after each tool call and each model turn. Displayed in the TUI
subagent sidebar.

### Task Resume

Claude Code doesn't support task resume (`task_id`). RARA won't either
for now — each `spawn_agent` creates a new session. This keeps
compatibility with Claude Code semantics.

### Display

TUI subagent sidebar shows:

```
  code-reviewer: reviewing PR #123            [12 tools · 5.2K tokens]
    Read src/tools/agent.rs
    Grep "permission" crates/
```

## Implementation Plan

1. Add `AgentDefinition` struct and YAML parsing.
2. Add `AgentDefinitionCache::load()` scanning `.rara/agents/` with
   `.claude/agents/` compatibility.
3. Extend `SubAgentKind` → `AgentDefinition` mapping.
4. Add tool filtering to subagent spawning.
5. Add `SubagentProgress` tracking.
6. Update TUI subagent display.
