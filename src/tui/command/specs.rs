// Spec constants reserved for inline command palette.

use crate::tui::state::{CommandSpec, LocalCommand, LocalCommandKind, TuiApp};

pub const COMMAND_SPECS: [CommandSpec; 18] = [
    CommandSpec {
        category: "Session",
        name: "permissions",
        usage: "/permissions",
        summary: "Choose a permission preset.",
        detail: "Open the permission picker and choose Auto, AcceptEdits, ReadOnly, or FullAccess. Review the selected preset before applying it.",
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
        name: "context",
        usage: "/context",
        summary: "Inspect the effective runtime context for the current turn.",
        detail: "Open a context modal that explains the effective prompt sources, active sections, workspace/runtime state, plan state, compaction metadata, and pending interaction inputs for the current turn.",
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
        summary: "Inspect loaded skills and invocation availability.",
        detail: "Open a read-only view of loaded skills and their runtime invocation availability. Runtime skill enablement cannot be changed from this view.",
    },
    CommandSpec {
        category: "Session",
        name: "goal",
        usage: "/goal",
        summary: "Manage the active thread goal.",
        detail: "Set a persistent objective that the agent keeps working toward across turns. A completed goal is replaced when a new objective is set.\n\n/goal                         show current goal status\n/goal --tokens <N> <objective> start goal with token budget N\n/goal <objective>             start goal with no budget\n/goal pause                   pause an active goal\n/goal resume                  resume a paused or blocked goal\n/goal clear                   clear the current goal",
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

    let kind = match canonical_command_name(name) {
        "quit" => LocalCommandKind::Quit,
        "help" => LocalCommandKind::Help,
        "status" => LocalCommandKind::Status,
        "context" => LocalCommandKind::Context,
        "clear" => LocalCommandKind::Clear,
        "resume" => LocalCommandKind::Resume,
        "plan" => LocalCommandKind::Plan,
        "approval" => LocalCommandKind::Approval,
        "compact" => LocalCommandKind::Compact,
        "tasks" => LocalCommandKind::Tasks,
        "model" => LocalCommandKind::Model,
        "connect" => LocalCommandKind::Connect,
        "mem" => LocalCommandKind::NowledgeMem,
        "review" => LocalCommandKind::Review,
        "mcp" => LocalCommandKind::Mcp,
        "skills" => LocalCommandKind::Skills,
        "permissions" => LocalCommandKind::Permissions,
        "goal" => LocalCommandKind::Goal,
        _ => return None,
    };

    Some(LocalCommand { kind, arg })
}

fn canonical_command_name(name: &str) -> &str {
    match name {
        "exit" => "quit",
        "runtime" => "status",
        "memory" => "context",
        "threads" => "resume",
        "task-list" => "tasks",
        "permission" => "permissions",
        _ => name,
    }
}

pub fn matching_commands(query: &str) -> Vec<&'static CommandSpec> {
    let query = query.to_ascii_lowercase();
    let query = canonical_command_name(&query);
    let mut candidates: Vec<_> = COMMAND_SPECS
        .iter()
        .filter_map(|spec| Some((command_score(spec, query)?, spec)))
        .collect();
    candidates.sort_by_key(|(score, spec)| (*score, spec.usage));
    candidates.into_iter().map(|(_, spec)| spec).collect()
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
    concat!(
        "Enter sends a message; while running, it queues a follow-up.\n\n",
        "/connect  Manage provider connections\n",
        "/model  Choose an available model\n",
        "/permissions  Choose a permission preset\n",
        "/plan  Enter read-only planning mode\n",
        "/status  Inspect runtime status\n",
        "/context  Inspect assembled context\n",
        "/compact  Summarize older conversation history\n",
        "/resume  Restore a recent thread\n\n",
        "Shift+Enter or Ctrl+J: insert a newline\n",
        "Esc: close an overlay, reject shell approval, or cancel a task\n",
        "Ctrl+C: cancel a task; otherwise clear the composer\n",
        "Up/Down: navigate lists; type to filter search pickers\n",
        "1/2/3: switch help tabs\n",
        "/quit or /exit: leave the TUI"
    )
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
