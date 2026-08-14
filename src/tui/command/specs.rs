// Spec constants reserved for inline command palette.

use crate::tui::state::{CommandSpec, LocalCommand, LocalCommandKind, TuiApp};

pub const COMMAND_SPECS: [CommandSpec; 21] = [
    CommandSpec {
        category: "Session",
        name: "permissions",
        usage: "/permissions",
        summary: "Cycle permission mode (Auto → AcceptEdits → ReadOnly → FullAccess).",
        detail: "Set what RARA can do without asking first. Cycles through Auto (sandbox workspace-write, bash always-approve), AcceptEdits (auto-approve file edits, suggestion bash), ReadOnly (plan mode, no edits), and FullAccess (network access, bash always-approve).",
    },
    CommandSpec {
        category: "Session",
        name: "help",
        usage: "/help",
        summary: "Show built-in commands and keyboard hints.",
        detail: "Open the help modal with general guidance, command references, and runtime details.",
    },
    CommandSpec {
        category: "Session",
        name: "status",
        usage: "/status",
        summary: "Show current provider, model, revision, workspace, and runtime counters.",
        detail: "Open a runtime status modal with provider, model, revision, workspace, session, token counters, and cache location.",
    },
    CommandSpec {
        category: "Session",
        name: "runtime",
        usage: "/runtime",
        summary: "Alias for /status.",
        detail: "Open the runtime status modal. This matches the runtime-focused naming used by other agent CLIs.",
    },
    CommandSpec {
        category: "Session",
        name: "context",
        usage: "/context",
        summary: "Inspect the effective runtime context for the current turn.",
        detail: "Open a context modal that explains the effective prompt sources, active sections, workspace/runtime state, plan state, compaction metadata, and pending interaction inputs for the current turn.",
    },
    CommandSpec {
        category: "Session",
        name: "memory",
        usage: "/memory",
        summary: "Alias for /context.",
        detail: "Open the context modal to inspect the effective assembled context, memory selection, and active runtime state.",
    },
    CommandSpec {
        category: "Session",
        name: "clear",
        usage: "/clear",
        summary: "Clear the visible transcript and keep the current backend.",
        detail: "Reset only the local transcript view. The current backend, session id, and active runtime remain unchanged.",
    },
    CommandSpec {
        category: "Session",
        name: "resume",
        usage: "/resume",
        summary: "Pick and restore a recent local thread.",
        detail: "Open the recent thread picker backed by the local thread store and rollout artifacts. This restores committed turns, plan state, and interaction cards for the selected thread.",
    },
    CommandSpec {
        category: "Session",
        name: "threads",
        usage: "/threads",
        summary: "Alias for /resume.",
        detail: "Open the recent thread picker and choose a local thread to restore.",
    },
    CommandSpec {
        category: "Session",
        name: "plan",
        usage: "/plan",
        summary: "Enter planning mode for the current task.",
        detail: "Switch the agent into read-only planning mode. In planning mode, inspection tools and read-only shell commands stay available, but editing, mutating shell commands, memory writes, and sub-agent launch tools are hidden or blocked. RARA can inspect the codebase, clarify constraints, refine the implementation approach, and only stop for approval once a concrete plan is ready. The agent can also enter planning mode automatically by calling enter_plan_mode.",
    },
    CommandSpec {
        category: "Session",
        name: "approval",
        usage: "/approval",
        summary: "Toggle bash approval between suggestion and always.",
        detail: "Toggle bash execution between suggestion-only mode and always-run mode. Suggestion mode keeps bash inside the plan/approval flow instead of executing immediately.",
    },
    CommandSpec {
        category: "Session",
        name: "compact",
        usage: "/compact",
        summary: "Compact the current conversation history immediately.",
        detail: "Force one explicit history compaction pass. Compaction summarizes older turns into a structured summary so the model can continue a long conversation without losing early context. Compaction runs on every message and tool-result batch, but /compact lets you trigger one on demand.",
    },
    CommandSpec {
        category: "Session",
        name: "tasks",
        usage: "/tasks [task_list_id]",
        summary: "Show or switch the active shared task list.",
        detail: "Without an argument, show the active shared task list and current task counts. With an argument, switch the active shared task list for runtime context, shared task tools, and future subagents.",
    },
    CommandSpec {
        category: "Session",
        name: "mcp",
        usage: "/mcp",
        summary: "Show configured MCP servers from the effective registry.",
        detail: "Load user config.toml and project .mcp.json, then show MCP servers grouped by scope and source path. This read-only status surface reports configured, disabled, and configuration failures without starting servers yet.",
    },
    CommandSpec {
        category: "Setup",
        name: "connect",
        usage: "/connect",
        summary: "Connect AI providers and add credentials. Supports multiple configured providers.",
        detail: "Open the provider list to pick an AI provider to connect to. Select a provider and follow the guided setup for API key or OAuth authentication.",
    },
    CommandSpec {
        category: "Setup",
        name: "model",
        usage: "/model",
        summary: "Open the unified model picker.",
        detail: "Open the unified model picker so you can browse all available models from every connected provider and switch the active model immediately.",
    },
    CommandSpec {
        category: "Setup",
        name: "mem",
        usage: "/mem",
        summary: "Configure the builtin Nowledge Mem local or cloud MCP connection.",
        detail: "Open the builtin Nowledge Mem configuration picker. Cloud mode stores only the endpoint and environment variable names; it never accepts or persists an API key value.",
    },
    CommandSpec {
        category: "Session",
        name: "review",
        usage: "/review",
        summary: "Compose a code review prompt with the current git diff.",
        detail: "Capture staged and unstaged git diff in the workspace and set up a review prompt that the agent can use to review the current changes. The diff is included as context in the prompt.",
    },
    CommandSpec {
        category: "Session",
        name: "quit",
        usage: "/quit",
        summary: "Exit the TUI session.",
        detail: "Leave the RARA TUI and restore the terminal. The /exit alias behaves the same way.",
    },
    CommandSpec {
        category: "Session",
        name: "skills",
        usage: "/skills",
        summary: "View and toggle loaded skills.",
        detail: "Open a skills picker that shows all loaded skills grouped by scope. Use space to toggle enable/disable. Changes take effect on next turn context assembly.",
    },
    CommandSpec {
        category: "Session",
        name: "goal",
        usage: "/goal",
        summary: "Manage the active thread goal.",
        detail: "Set a persistent objective that the agent keeps working toward across turns.\n\n/goal                         show current goal status\n/goal --tokens <N> <objective> start goal with token budget N\n/goal <objective>             start goal with no budget\n/goal pause                   pause an active goal\n/goal resume                  resume a paused goal\n/goal clear                   clear the current goal",
    },
];

pub fn parse_local_command(input: &str) -> Option<LocalCommand> {
    let trimmed = input.trim();
    let command = trimmed.strip_prefix('/')?;
    let mut parts = command.splitn(2, char::is_whitespace);
    let name = parts.next()?.trim();
    let arg = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let kind = match name {
        "quit" | "exit" => LocalCommandKind::Quit,
        "help" => LocalCommandKind::Help,
        "status" | "runtime" => LocalCommandKind::Status,
        "context" | "memory" => LocalCommandKind::Context,
        "clear" => LocalCommandKind::Clear,
        "resume" | "threads" => LocalCommandKind::Resume,
        "plan" => LocalCommandKind::Plan,
        "approval" => LocalCommandKind::Approval,
        "compact" => LocalCommandKind::Compact,
        "tasks" | "task-list" => LocalCommandKind::Tasks,
        "model" => LocalCommandKind::Model,
        "connect" => LocalCommandKind::Connect,
        "mem" => LocalCommandKind::NowledgeMem,
        "review" => LocalCommandKind::Review,
        "mcp" => LocalCommandKind::Mcp,
        "skills" => LocalCommandKind::Skills,
        "permissions" | "permission" => LocalCommandKind::Permissions,
        "goal" => LocalCommandKind::Goal,
        _ => return None,
    };

    Some(LocalCommand { kind, arg })
}

pub fn matching_commands(query: &str) -> Vec<&'static CommandSpec> {
    let mut candidates: Vec<_> = COMMAND_SPECS
        .iter()
        .filter_map(|spec| Some((command_score(spec, query)?, spec)))
        .collect();
    candidates.sort_by_key(|(score, spec)| (*score, spec.usage));
    candidates.into_iter().map(|(_, spec)| spec).collect()
}

#[cfg(test)]
pub fn command_spec_by_name(name: &str) -> Option<&'static CommandSpec> {
    COMMAND_SPECS.iter().find(|spec| spec.name == name)
}

#[cfg(test)]
pub fn recommended_commands(app: &TuiApp) -> Vec<&'static CommandSpec> {
    let names = if app.is_busy() {
        vec!["context", "help", "status"]
    } else {
        let mut n = vec!["context", "help", "model", "resume", "status"];
        if !app.committed_turns.is_empty() || !app.active_turn.entries.is_empty() {
            n.push("compact");
            n.push("plan");
        }
        n
    };
    names
        .iter()
        .filter_map(|name| command_spec_by_name(name))
        .collect()
}

pub fn palette_commands(_app: &TuiApp, query: &str) -> Vec<&'static CommandSpec> {
    if !query.trim().is_empty() {
        return matching_commands(query);
    }

    let mut commands = COMMAND_SPECS.iter().collect::<Vec<_>>();
    commands.sort_by_key(|spec| spec.name);
    commands
}

pub fn palette_command_by_index(
    app: &TuiApp,
    query: &str,
    index: usize,
) -> Option<&'static CommandSpec> {
    palette_commands(app, query).get(index).copied()
}

pub fn general_help_text() -> &'static str {
    "RARA uses a single composer as the control surface.\n\nNormal input goes to the current agent.\nSlash commands stay local and open overlays or update runtime state.\n\nCompaction:\n  /compact forces one history compaction pass\n\nContext:\n  /context shows the effective runtime context for the current turn\n\nModes:\n  /permissions cycles through permission presets (auto, accept-edits, read-only, full-access)\n  /plan enters planning mode for the current task\n  The agent may call enter_plan_mode for non-trivial repository work\n  /approval toggles bash approval between suggestion and always\n\nAuth:\n  /login opens the provider auth picker\n  /logout clears the saved provider credential\n\nEditing:\n  apply_patch is the default tool for updating existing files\n  replace_lines is for verified large line-range edits\n  write_file is for new files or full rewrites\n  replace is only a simple fallback for unique string swaps\n\nKeyboard:\n  Enter submit current composer input\n  Shift+Enter insert a newline in the composer\n  Esc close the current overlay only\n  Up/Down or j/k move inside lists\n  1/2/3 switch help tabs or choose guided model options\n\nExit:\n  /quit or /exit leave the TUI."
}

fn command_score(spec: &CommandSpec, query: &str) -> Option<u8> {
    if query.is_empty() {
        return Some(0);
    }
    let query = query.to_ascii_lowercase();
    let name = spec.name.to_ascii_lowercase();
    let usage = spec.usage.to_ascii_lowercase();
    let summary = spec.summary.to_ascii_lowercase();

    if name == query {
        Some(0)
    } else if name.starts_with(&query) {
        Some(1)
    } else if usage.contains(&query) {
        Some(2)
    } else if summary.contains(&query) {
        Some(3)
    } else {
        subsequence_match(&name, &query).then_some(4)
    }
}

fn subsequence_match(haystack: &str, needle: &str) -> bool {
    let mut chars = needle.chars();
    let mut current = chars.next();
    for ch in haystack.chars() {
        if Some(ch) == current {
            current = chars.next();
            if current.is_none() {
                return true;
            }
        }
    }
    current.is_none()
}

#[cfg(test)]
pub fn help_text() -> String {
    let mut specs = COMMAND_SPECS.iter().collect::<Vec<_>>();
    specs.sort_by_key(|spec| spec.name);
    let commands = specs
        .into_iter()
        .map(|spec| format!("  {}  {}", spec.usage, spec.summary))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Built-in commands:\n{}\n\nCompaction:\n  /compact   summarize older conversation history now\n\nThreads:\n  /resume    reopen a recent local thread\n\nModes:\n  /permissions   cycle permission presets (auto, accept-edits, read-only, full-access)\n  /plan      enter planning mode for the current task\n  Agent may call enter_plan_mode automatically\n  /approval  toggle bash approval mode\n\nProvider setup:\n  /connect   manage provider credentials and connection settings\n  /model     choose a model from available providers\n\nEditing:\n  apply_patch    preferred for editing existing files\n  replace_lines  use for verified large line-range edits\n  write_file     use for new files or full rewrites\n  replace        simple fallback for unique string replacement\n\nKeyboard:\n  Enter submit\n  Shift+Enter insert newline\n  Esc close current overlay\n\nExit:\n  /quit\n  /exit",
        commands
    )
}

#[cfg(test)]
pub fn normalize_command_token(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}
