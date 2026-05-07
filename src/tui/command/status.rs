use time::{OffsetDateTime, format_description};

use crate::config::RaraConfig;
use crate::context::{CacheStatus, RetrievalCandidateContextEntry, RetrievalProviderStatus};
use crate::tui::format::cache_hit_rate_label;
use crate::tui::session_restore::provider_requires_api_key;
use crate::tui::state::{
    PROVIDER_FAMILIES, PendingInteractionSnapshot, ProviderFamily, TuiApp, current_model_presets,
};

fn format_pending_interaction(snapshot: &PendingInteractionSnapshot) -> String {
    let kind = match snapshot.kind {
        crate::tui::state::InteractionKind::RequestInput => "request_input",
        crate::tui::state::InteractionKind::Approval => "approval",
        crate::tui::state::InteractionKind::PlanApproval => "plan_approval",
    };
    let mut lines = vec![format!(
        "- kind={kind} title={} summary={}",
        snapshot.title,
        if snapshot.summary.is_empty() {
            "-"
        } else {
            snapshot.summary.as_str()
        }
    )];
    if !snapshot.options.is_empty() {
        lines.push(format!("  options={}", snapshot.options.len()));
    }
    if let Some(source) = snapshot.source.as_deref() {
        lines.push(format!("  source={source}"));
    }
    if let Some(note) = snapshot.note.as_deref() {
        lines.push(format!("  note={note}"));
    }
    lines.join("\n")
}

fn render_context_assembly_entries(app: &TuiApp, layer: &str, title: &str) -> String {
    let entries: Vec<&crate::context::ContextAssemblyEntry> = app
        .snapshot
        .assembly_entries
        .iter()
        .filter(|entry| entry.layer == layer)
        .collect::<Vec<_>>();
    let count = entries.len();
    if entries.is_empty() {
        return format!("{title}\n  (none)");
    }

    let body = entries
        .into_iter()
        .enumerate()
        .map(|(idx, entry)| {
            let (connector, vertical) = if idx + 1 == count {
                ("└──", " ")
            } else {
                ("├──", "│")
            };
            let path = entry
                .source_path
                .as_deref()
                .filter(|v| !v.is_empty())
                .unwrap_or("-");
            let injected = if entry.injected { "" } else { " (not injected)" };
            let cache = cache_marker(entry.cache_status);
            let budget = entry
                .budget_impact_tokens
                .map(|v| format!(" {}", format_token_count(v)))
                .unwrap_or_default();
            let drop_note = match entry.dropped_reason.as_ref() {
                Some(r) if !r.is_empty() && r != "-" => format!(" ── reason: {r}"),
                _ => String::new(),
            };
            format!(
                "  {connector} {cache}[{kind}] {label}{injected}{budget}\n  {vertical}   path: {path}{drop_note}",
                kind = entry.kind,
                label = entry.label,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!("{title}\n{body}")
}

fn render_retrieval_orchestration(app: &TuiApp) -> String {
    let view = &app.snapshot.retrieval_orchestration;
    let budget = view
        .budget
        .selection_budget_tokens
        .map(format_token_count)
        .unwrap_or_else(|| "unlimited".to_string());
    let providers = render_retrieval_providers(view.providers.as_slice());
    let selected = render_retrieval_candidates(view.selected.as_slice());
    let available = render_retrieval_candidates(view.available.as_slice());
    let dropped = render_retrieval_candidates(view.dropped.as_slice());

    format!(
        "Retrieval Orchestration  (budget: {budget}; selected: {selected_tokens}; available: {available_tokens}; dropped: {dropped_tokens})\n  query: {query}\n  providers:\n{providers}\n  candidates:\n    selected:\n{selected}\n    available:\n{available}\n    dropped:\n{dropped}",
        selected_tokens = format_token_count(view.budget.selected_tokens),
        available_tokens = format_token_count(view.budget.available_tokens),
        dropped_tokens = format_token_count(view.budget.dropped_tokens),
        query = if view.query.trim().is_empty() {
            "-"
        } else {
            view.query.as_str()
        },
    )
}

fn render_retrieval_providers(providers: &[RetrievalProviderStatus]) -> String {
    if providers.is_empty() {
        return "    (none)".to_string();
    }

    providers
        .iter()
        .enumerate()
        .map(|(idx, provider)| {
            let (connector, vertical) = if idx + 1 == providers.len() {
                ("└──", " ")
            } else {
                ("├──", "│")
            };
            let detail = truncate_preview(provider.detail.as_str(), 70);
            let reason = truncate_preview(provider.inclusion_reason.as_str(), 80);
            format!(
                "    {connector} [{kind}] {label} status={status}\n    {vertical}   detail: {detail}\n    {vertical}   reason: {reason}",
                kind = provider.kind,
                label = provider.label,
                status = provider.status,
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_retrieval_candidates(candidates: &[RetrievalCandidateContextEntry]) -> String {
    if candidates.is_empty() {
        return "      (none)".to_string();
    }

    candidates
        .iter()
        .enumerate()
        .map(|(idx, candidate)| {
            let (connector, vertical) = if idx + 1 == candidates.len() {
                ("└──", " ")
            } else {
                ("├──", "│")
            };
            let budget = candidate
                .budget_impact_tokens
                .map(|v| format!(" {}", format_token_count(v)))
                .unwrap_or_default();
            let detail = truncate_preview(candidate.detail.as_str(), 60);
            let reason = truncate_preview(candidate.reason.as_str(), 80);
            format!(
                "      {connector} [{source_kind}/{kind}] {label}{budget}\n      {vertical}   detail: {detail}\n      {vertical}   reason: {reason}",
                source_kind = candidate.source_kind,
                kind = candidate.kind,
                label = candidate.label,
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn truncate_preview(text: &str, max_len: usize) -> String {
    let condensed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if condensed.chars().count() <= max_len {
        return condensed;
    }
    if max_len == 0 {
        return String::new();
    }
    if max_len == 1 {
        return "…".to_string();
    }

    let keep_chars = max_len - 1;
    let truncate_at = condensed
        .char_indices()
        .nth(keep_chars)
        .map_or(condensed.len(), |(idx, _)| idx);
    format!("{}…", &condensed[..truncate_at])
}

fn cache_marker(cache_status: Option<CacheStatus>) -> &'static str {
    match cache_status {
        Some(CacheStatus::Hit) => "● ",
        Some(CacheStatus::Miss) => "○ ",
        Some(CacheStatus::NoCache) => "- ",
        None => "",
    }
}

fn format_token_count(tokens: usize) -> String {
    format!("{tokens} tokens")
}

fn format_token_percent(tokens: usize, window: Option<usize>) -> String {
    let Some(window) = window.filter(|value| *value > 0) else {
        return String::new();
    };
    let percent = tokens as f64 * 100.0 / window as f64;
    format!(" ({percent:.1}%)")
}

fn context_usage_line(label: &str, tokens: usize, window: Option<usize>) -> String {
    format!(
        "  {label}: {}{}",
        format_token_count(tokens),
        format_token_percent(tokens, window)
    )
}

fn render_context_usage_summary(app: &TuiApp) -> String {
    let window = app.snapshot.context_window_tokens;
    let prompt_tokens = app
        .snapshot
        .stable_instructions_budget
        .saturating_add(app.snapshot.workspace_prompt_budget);
    let used_tokens = prompt_tokens
        .saturating_add(app.snapshot.active_turn_budget)
        .saturating_add(app.snapshot.compacted_history_budget)
        .saturating_add(app.snapshot.retrieved_memory_budget)
        .saturating_add(app.snapshot.reserved_output_tokens);
    let free_tokens = app
        .snapshot
        .remaining_input_budget
        .or_else(|| window.map(|window| window.saturating_sub(used_tokens)));
    let autocompact_buffer = window
        .map(|window| window.saturating_sub(app.snapshot.compact_threshold_tokens))
        .filter(|tokens| *tokens > 0);

    let mut lines = vec![
        "Context Usage".to_string(),
        format!("  model: {}", app.current_model_label()),
        format!(
            "  used: {}{} / {}",
            format_token_count(used_tokens),
            format_token_percent(used_tokens, window),
            window
                .map(format_token_count)
                .unwrap_or_else(|| "unknown window".to_string())
        ),
        "  Estimated usage by category".to_string(),
        context_usage_line("System prompt", prompt_tokens, window),
        context_usage_line("Active turn", app.snapshot.active_turn_budget, window),
        context_usage_line(
            "Compacted history",
            app.snapshot.compacted_history_budget,
            window,
        ),
        context_usage_line(
            "Retrieved memory",
            app.snapshot.retrieved_memory_budget,
            window,
        ),
        context_usage_line(
            "Reserved output",
            app.snapshot.reserved_output_tokens,
            window,
        ),
    ];
    if let Some(tokens) = free_tokens {
        lines.push(context_usage_line("Free space", tokens, window));
    }
    if let Some(tokens) = autocompact_buffer {
        lines.push(context_usage_line("Autocompact buffer", tokens, window));
    }
    lines.join("\n")
}

fn format_basis_points(value: Option<u32>) -> String {
    value
        .map(|basis_points| format!("{:.1}%", basis_points as f64 / 100.0))
        .unwrap_or_else(|| "-".to_string())
}

fn format_char_count(chars: usize) -> String {
    format!("{chars} chars")
}

fn render_context_observability(app: &TuiApp) -> String {
    let view = &app.snapshot.context_observability;
    format!(
        "Observability\n  cache: hit={} miss={} hit_rate={}\n  microcompact: enabled={} cleared={} kept={} saved={} budget={} keep_recent={}\n  retrieval: providers={} candidates={} selected={} available={} dropped={} selected_budget={}\n  agent_turn: idx={} mode={} stop={} outcome={} phase={} text={} reasoning={} reasoning_only={} streamed_text={} streamed_reasoning={} assistant_recorded={} tools={} consecutive_reasoning_only={}",
        format_token_count(view.cache.hit_tokens as usize),
        format_token_count(view.cache.miss_tokens as usize),
        format_basis_points(view.cache.hit_rate_basis_points),
        view.microcompact.enabled,
        view.microcompact.cleared_results,
        view.microcompact.kept_results,
        format_char_count(view.microcompact.saved_chars),
        format_char_count(view.microcompact.budget_chars),
        view.microcompact.keep_recent,
        view.retrieval.provider_count,
        view.retrieval.candidate_count,
        view.retrieval.selected_count,
        view.retrieval.available_count,
        view.retrieval.dropped_count,
        format_token_count(view.retrieval.selected_tokens),
        view.agent_turn.agentic_turn_index,
        view.agent_turn.execution_mode,
        view.agent_turn.model_stop_reason.as_deref().unwrap_or("-"),
        view.agent_turn.loop_outcome.as_deref().unwrap_or("-"),
        view.agent_turn.continuation_phase.as_deref().unwrap_or("-"),
        view.agent_turn.had_text_response,
        view.agent_turn.had_reasoning_response,
        view.agent_turn.reasoning_only,
        view.agent_turn.streamed_text_delta,
        view.agent_turn.streamed_reasoning_delta,
        view.agent_turn.assistant_message_recorded,
        view.agent_turn.tool_call_count,
        view.agent_turn.consecutive_reasoning_only_turns,
    )
}

fn todo_summary_line(app: &TuiApp) -> String {
    let summary = &app.snapshot.todo.summary;
    if summary.total == 0 {
        return "none".to_string();
    }
    let active = summary.active_item.as_deref().unwrap_or("-");
    format!(
        "{} total, {} pending, {} in_progress, {} completed, {} cancelled, active={}",
        summary.total,
        summary.pending,
        summary.in_progress,
        summary.completed,
        summary.cancelled,
        truncate_preview(active, 80)
    )
}

fn render_todo_context(app: &TuiApp) -> String {
    let summary = &app.snapshot.todo.summary;
    if summary.total == 0 {
        return "Todo\n  artifact: -\n  items: none".to_string();
    }
    let artifact = app.snapshot.todo_artifact_path.as_deref().unwrap_or("-");
    let updated_at = app
        .snapshot
        .todo
        .updated_at
        .map(format_unix_timestamp_utc)
        .unwrap_or_else(|| "-".to_string());
    let active = summary.active_item.as_deref();
    let items = if app.snapshot.todo.items.is_empty() {
        "  items: none".to_string()
    } else {
        let rendered_items = app
            .snapshot
            .todo
            .items
            .iter()
            .take(8)
            .map(|(id, status, content)| {
                let suffix = if active == Some(content.as_str()) {
                    format!("{id}, active")
                } else {
                    id.to_string()
                };
                format!(
                    "    - [{status}] {} ({suffix})",
                    truncate_preview(content, 100)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let omitted = app.snapshot.todo.items.len().saturating_sub(8);
        if omitted == 0 {
            format!("  items:\n{rendered_items}")
        } else {
            format!("  items:\n{rendered_items}\n    ... {omitted} more")
        }
    };
    format!(
        "Todo\n  artifact: {artifact}\n  updated_at: {updated_at}\n  total: {}  pending: {}  in_progress: {}  completed: {}  cancelled: {}\n{}",
        summary.total,
        summary.pending,
        summary.in_progress,
        summary.completed,
        summary.cancelled,
        items,
    )
}

fn format_unix_timestamp_utc(timestamp: i64) -> String {
    let format = format_description::parse("[year]-[month]-[day] [hour]:[minute]:[second] UTC")
        .expect("static timestamp format should be valid");

    OffsetDateTime::from_unix_timestamp(timestamp)
        .ok()
        .and_then(|dt| dt.format(&format).ok())
        .unwrap_or_else(|| "invalid timestamp".to_string())
}

pub fn status_context_text(app: &TuiApp) -> String {
    let prompt_warnings = if app.snapshot.prompt_warnings.is_empty() {
        None
    } else {
        Some(format!(
            "Warnings:\n{}",
            app.snapshot
                .prompt_warnings
                .iter()
                .map(|warning| format!("  - {warning}"))
                .collect::<Vec<_>>()
                .join("\n")
        ))
    };
    let plan_lines = if app.snapshot.plan_steps.is_empty() {
        "  - No active plan steps.".to_string()
    } else {
        app.snapshot
            .plan_steps
            .iter()
            .map(|(status, step)| format!("  - [{status}] {step}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let pending_interactions = if app.snapshot.pending_interactions.is_empty() {
        "  - None.".to_string()
    } else {
        app.snapshot
            .pending_interactions
            .iter()
            .map(format_pending_interaction)
            .map(|entry| format!("  {entry}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let last_boundary = if let Some(version) = app.snapshot.last_compaction_boundary_version {
        let before = app
            .snapshot
            .last_compaction_boundary_before_tokens
            .map(format_token_count)
            .unwrap_or_else(|| "-".to_string());
        let files = app
            .snapshot
            .last_compaction_boundary_recent_file_count
            .map(|count| count.to_string())
            .unwrap_or_else(|| "-".to_string());
        format!("v{version} before={before} recent_file_count={files}")
    } else {
        "-".to_string()
    };
    let mut sections = vec![
        render_context_usage_summary(app),
        format!(
            "Session\n  cwd: {}\n  branch: {}\n  session: {}\n  history: {} msgs  {} entries",
            app.snapshot.cwd,
            app.snapshot.branch,
            app.snapshot.session_id,
            app.snapshot.history_len,
            app.transcript_entry_count(),
        ),
        format!(
            "Budget\n  window: {}  reserved: {}  remaining: {}\n  stable: {}  workspace: {}  active: {}  compacted: {}  retrieved: {}",
            app.snapshot
                .context_window_tokens
                .map(format_token_count)
                .unwrap_or_else(|| "-".to_string()),
            format_token_count(app.snapshot.reserved_output_tokens),
            app.snapshot
                .remaining_input_budget
                .map(format_token_count)
                .unwrap_or_else(|| "-".to_string()),
            format_token_count(app.snapshot.stable_instructions_budget),
            format_token_count(app.snapshot.workspace_prompt_budget),
            format_token_count(app.snapshot.active_turn_budget),
            format_token_count(app.snapshot.compacted_history_budget),
            format_token_count(app.snapshot.retrieved_memory_budget),
        ),
        format!(
            "Compaction\n  estimated: {}  threshold: {}  count: {}\n  last: {} → {}  boundary: {}",
            format_token_count(app.snapshot.estimated_history_tokens),
            format_token_count(app.snapshot.compact_threshold_tokens),
            app.snapshot.compaction_count,
            app.snapshot
                .last_compaction_before_tokens
                .map(format_token_count)
                .unwrap_or_else(|| "-".to_string()),
            app.snapshot
                .last_compaction_after_tokens
                .map(format_token_count)
                .unwrap_or_else(|| "-".to_string()),
            last_boundary
        ),
        render_context_observability(app),
        format!(
            "Plan\n  mode: {}  explanation: {}\n{}",
            app.agent_execution_mode_label(),
            app.snapshot.plan_explanation.as_deref().unwrap_or("-"),
            plan_lines
        ),
        render_todo_context(app),
        format!("Pending\n{}", pending_interactions),
        render_context_assembly_entries(app, "stable_instructions", "Stable Instructions"),
        render_context_assembly_entries(
            app,
            "workspace_prompt_sources",
            "Workspace Prompt Sources",
        ),
        render_context_assembly_entries(app, "active_memory_inputs", "Active Memory Inputs"),
        render_retrieval_orchestration(app),
        render_context_assembly_entries(app, "compacted_history", "Compacted History"),
        render_context_assembly_entries(app, "active_turn_state", "Active Turn State"),
        render_context_assembly_entries(app, "retrieval_ready", "Retrieval-ready"),
    ];
    if let Some(warnings) = prompt_warnings {
        sections.insert(2, warnings);
    }
    sections.join("\n\n")
}

pub fn status_runtime_text(app: &TuiApp) -> String {
    let mode = if provider_requires_api_key(&app.config.provider) {
        "hosted"
    } else {
        "local"
    };
    let phase = app.runtime_phase_label();
    let detail = app.runtime_phase_detail.as_deref().unwrap_or("-");
    let thinking = if app.config.provider == "ollama" || app.config.provider == "ollama-native" {
        if app.config.thinking.unwrap_or(true) {
            "on"
        } else {
            "off"
        }
    } else {
        "-"
    };
    let surface = app.config.effective_provider_surface();
    let reasoning_summary = surface
        .reasoning_summary
        .display_or(rara_config::DEFAULT_REASONING_SUMMARY);
    let reasoning_effort_label = app.current_reasoning_effort_label();
    let endpoint_profile = if app.config.provider == "openai-compatible" {
        app.config.active_openai_profile_label().unwrap_or("-")
    } else {
        "-"
    };
    let endpoint_kind = if app.config.provider == "openai-compatible" {
        app.config
            .active_openai_profile_kind()
            .map(|kind| match kind {
                rara_config::OpenAiEndpointKind::Custom => "custom",
                rara_config::OpenAiEndpointKind::Deepseek => "deepseek",
                rara_config::OpenAiEndpointKind::Kimi => "kimi",
                rara_config::OpenAiEndpointKind::Openrouter => "openrouter",
            })
            .unwrap_or("-")
    } else {
        "-"
    };
    let codex_auth_mode = match app.codex_auth_mode {
        Some(crate::oauth::SavedCodexAuthMode::ApiKey) => "api_key",
        Some(crate::oauth::SavedCodexAuthMode::Chatgpt) => "chatgpt",
        None => "-",
    };
    let codex_endpoint_kind = if app.config.provider == "codex" {
        match app.codex_auth_mode {
            Some(crate::oauth::SavedCodexAuthMode::Chatgpt) => "chatgpt_codex",
            Some(crate::oauth::SavedCodexAuthMode::ApiKey) => "openai_api",
            None => "unknown",
        }
    } else {
        "-"
    };
    let (device, dtype) = if is_local_provider(&app.config.provider) {
        crate::local_backend::local_runtime_target()
            .unwrap_or_else(|_| ("unavailable".to_string(), "unavailable".to_string()))
    } else {
        ("remote".to_string(), "-".to_string())
    };
    format!(
        "provider={}\nendpoint_profile={}\nendpoint_kind={}\nmodel={}\nmodel_source={}\nbase_url={}\nbase_url_source={}\nrevision={}\nrevision_source={}\nagent_mode={}\nbash_approval={}\nmode={}\napi_key={}\napi_key_source={}\ncodex_auth_mode={}\ncodex_endpoint_kind={}\nthinking={}\nreasoning_summary={}\nreasoning_summary_source={}\nreasoning_effort={}\nreasoning_effort_source={}\ntodo={}\ndevice={}\ndtype={}\nfocused={}\nphase={}\ndetail={}",
        surface.provider,
        endpoint_profile,
        endpoint_kind,
        surface.model.display_or(app.current_model_label()),
        surface.model.source.label(),
        surface.base_url.display_or("-"),
        surface.base_url.source.label(),
        surface.revision.display_or("main"),
        surface.revision.source.label(),
        app.agent_execution_mode_label(),
        app.bash_approval_mode_label(),
        mode,
        api_key_status(&app.config),
        surface.api_key.source.label(),
        codex_auth_mode,
        codex_endpoint_kind,
        thinking,
        reasoning_summary,
        surface.reasoning_summary.source.label(),
        surface
            .reasoning_effort
            .display_or(reasoning_effort_label.as_str()),
        surface.reasoning_effort.source.label(),
        todo_summary_line(app),
        device,
        dtype,
        app.terminal_focused,
        phase,
        detail,
    )
}

pub fn status_workspace_text(app: &TuiApp) -> String {
    format!(
        "workspace={}\nbranch={}\nsession={}\nmessages={}\ntranscript={}",
        app.snapshot.cwd,
        app.snapshot.branch,
        app.snapshot.session_id,
        app.snapshot.history_len,
        app.transcript_entry_count(),
    )
}

pub fn status_resources_text(app: &TuiApp) -> String {
    let cache = if is_local_provider(&app.config.provider) {
        crate::local_backend::default_local_model_cache_dir()
            .display()
            .to_string()
    } else {
        "-".to_string()
    };
    let state_db = app.state_db_status.as_deref().unwrap_or("-");
    let context = match app.snapshot.context_window_tokens {
        Some(window) => format!(
            "{} / {} (~auto @ {}, reserve {})",
            format_token_count(app.snapshot.estimated_history_tokens),
            format_token_count(window),
            format_token_count(app.snapshot.compact_threshold_tokens),
            format_token_count(app.snapshot.reserved_output_tokens)
        ),
        None => format!(
            "{} (~auto @ {})",
            format_token_count(app.snapshot.estimated_history_tokens),
            format_token_count(app.snapshot.compact_threshold_tokens)
        ),
    };
    let last_compact = match (
        app.snapshot.last_compaction_before_tokens,
        app.snapshot.last_compaction_after_tokens,
    ) {
        (Some(before), Some(after)) => format!(
            "{} -> {}",
            format_token_count(before),
            format_token_count(after)
        ),
        _ => "-".to_string(),
    };
    let last_compact_ratio = app
        .snapshot
        .last_compaction_before_tokens
        .zip(app.snapshot.last_compaction_after_tokens)
        .map(|(before, after)| {
            if before > 0 {
                format!("{:.1}%", (after as f64 / before as f64) * 100.0)
            } else {
                "-".to_string()
            }
        })
        .unwrap_or_else(|| "-".to_string());
    let compact_boundary = app
        .snapshot
        .last_compaction_boundary_recent_file_count
        .map(|count| format!("{count} files"))
        .unwrap_or_else(|| "-".to_string());
    let recent_compact_file_count = app.snapshot.last_compaction_recent_files.len().to_string();
    let recent_compact_files = if app.snapshot.last_compaction_recent_files.is_empty() {
        "-".to_string()
    } else {
        app.snapshot.last_compaction_recent_files.join(", ")
    };
    let retrieval_budget = app
        .snapshot
        .memory_selection
        .selection_budget_tokens
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string());
    let retrieval_selected = if app.snapshot.memory_selection.selected_items.is_empty() {
        "-".to_string()
    } else {
        app.snapshot
            .memory_selection
            .selected_items
            .iter()
            .map(|item| item.kind.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let cache_hit_rate = cache_hit_rate_label(
        app.snapshot.total_cache_hit_tokens,
        app.snapshot.total_cache_miss_tokens,
    )
    .unwrap_or_else(|| "-".to_string());
    format!(
        "tokens={} in / {} out\ncache_hit_tokens={}\ncache_miss_tokens={}\ncache_hit_rate={}\ncontext_estimate={}\nretrieval_budget={}\nretrieval_selected={}\ncompactions={} (last: {}, ratio: {})\ncompact_boundary={}\nrecent_compact_file_count={}\nrecent_compact_files={}\ncache={}\nstate_db={}",
        app.snapshot.total_input_tokens,
        app.snapshot.total_output_tokens,
        app.snapshot.total_cache_hit_tokens,
        app.snapshot.total_cache_miss_tokens,
        cache_hit_rate,
        context,
        retrieval_budget,
        retrieval_selected,
        app.snapshot.compaction_count,
        last_compact,
        last_compact_ratio,
        compact_boundary,
        recent_compact_file_count,
        recent_compact_files,
        cache,
        state_db,
    )
}

pub fn status_prompt_sources_text(app: &TuiApp) -> String {
    let mut lines = vec![
        format!("base prompt: {}", app.snapshot.prompt_base_kind),
        format!(
            "active sections: {}",
            app.snapshot.prompt_section_keys.join(", ")
        ),
        String::new(),
    ];
    if app.snapshot.prompt_source_entries.is_empty() {
        lines.push("no structured prompt sources discovered".to_string());
    } else {
        lines.extend(app.snapshot.prompt_source_entries.iter().map(|entry| {
            format!(
                "{}. {} [{}] {}\n   why: {}",
                entry.order, entry.label, entry.kind, entry.display_path, entry.inclusion_reason
            )
        }));
    }
    if !app.snapshot.prompt_warnings.is_empty() {
        if lines.len() > 3 {
            lines.push(String::new());
        }
        lines.push("warnings:".to_string());
        lines.extend(
            app.snapshot
                .prompt_warnings
                .iter()
                .map(|warning| format!("- {warning}")),
        );
    }

    if app.snapshot.prompt_source_entries.is_empty() && app.snapshot.prompt_warnings.is_empty() {
        "No prompt sources discovered.".to_string()
    } else {
        lines.join("\n")
    }
}

pub fn download_status_text(app: &TuiApp) -> Option<String> {
    if !matches!(
        app.runtime_phase,
        crate::tui::state::RuntimePhase::RebuildingBackend
            | crate::tui::state::RuntimePhase::BackendReady
    ) {
        return None;
    }

    let cache = if is_local_provider(&app.config.provider) {
        crate::local_backend::default_local_model_cache_dir()
            .display()
            .to_string()
    } else {
        "-".to_string()
    };
    let stage = app.runtime_phase_detail.as_deref().unwrap_or("waiting");
    let current_stage = infer_download_stage(stage);
    let steps = [
        ("setup", "Prepare request"),
        ("cache", "Resolve cache"),
        ("manifest", "Resolve manifest"),
        ("artifact", "Fetch tokenizer/config"),
        ("weights", "Fetch weights"),
        ("runtime", "Initialize runtime"),
        ("ready", "Model ready"),
    ]
    .into_iter()
    .enumerate()
    .map(|(idx, (key, label))| {
        let marker = if key == current_stage {
            ">"
        } else if download_stage_index(key) < download_stage_index(current_stage) {
            "x"
        } else {
            " "
        };
        format!("[{marker}] {}. {label}", idx + 1)
    })
    .collect::<Vec<_>>()
    .join("\n");
    Some(format!(
        "model={}\ncurrent={}\ncache={}\n\nsteps:\n{}",
        app.current_model_label(),
        stage,
        cache,
        steps,
    ))
}

fn infer_download_stage(detail: &str) -> &'static str {
    if detail.starts_with("Ready") {
        "ready"
    } else if detail.starts_with("Initializing") {
        "runtime"
    } else if detail.starts_with("Fetching weights") {
        "weights"
    } else if detail.starts_with("Fetching tokenizer") || detail.starts_with("Fetching config") {
        "artifact"
    } else if detail.starts_with("Resolving manifest") {
        "manifest"
    } else if detail.starts_with("Resolving cache") {
        "cache"
    } else {
        "setup"
    }
}

fn download_stage_index(stage: &str) -> usize {
    match stage {
        "setup" => 0,
        "cache" => 1,
        "manifest" => 2,
        "artifact" => 3,
        "weights" => 4,
        "runtime" => 5,
        "ready" => 6,
        _ => 0,
    }
}

pub fn recent_transcript_preview(app: &TuiApp, limit: usize) -> String {
    let mut entries = app
        .committed_turns
        .iter()
        .flat_map(|turn| turn.entries.iter())
        .chain(app.active_turn.entries.iter())
        .collect::<Vec<_>>();
    if entries.is_empty() {
        return "No transcript entries yet.".to_string();
    }
    entries
        .drain(..)
        .rev()
        .take(limit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|entry| {
            let first_line = entry.message.lines().next().unwrap_or("").trim();
            let preview = if first_line.is_empty() {
                "(empty)"
            } else {
                first_line
            };
            format!("{}: {preview}", entry.role)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn current_turn_preview(app: &TuiApp, limit: usize) -> String {
    if app.active_turn.entries.is_empty() {
        return "No active turn yet.".to_string();
    }

    app.active_turn
        .entries
        .iter()
        .take(limit)
        .map(|entry| {
            let first_line = entry.message.lines().next().unwrap_or("").trim();
            let preview = if first_line.is_empty() {
                "(empty)"
            } else {
                first_line
            };
            format!("{}: {preview}", entry.role)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn model_help_text(app: &TuiApp) -> String {
    let lines = PROVIDER_FAMILIES
        .iter()
        .enumerate()
        .map(|(provider_idx, (family, title, _))| {
            let provider_lines = if matches!(family, ProviderFamily::Codex) {
                if app.codex_model_options.is_empty() {
                    "  Sign in to load the current Codex model catalog.".to_string()
                } else {
                    app.codex_model_options
                        .iter()
                        .enumerate()
                        .map(|(idx, preset)| {
                            let marker = if app.config.provider == "codex"
                                && app.config.model.as_deref() == Some(preset.model.as_str())
                            {
                                "*"
                            } else {
                                " "
                            };
                            format!("{marker} {}. {} ({})", idx + 1, preset.label, preset.model)
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            } else if matches!(family, ProviderFamily::DeepSeek) {
                app.deepseek_model_options
                    .iter()
                    .enumerate()
                    .map(|(idx, model)| {
                        let marker = if app.config.active_openai_profile_kind()
                            == Some(rara_config::OpenAiEndpointKind::Deepseek)
                            && app.config.model.as_deref() == Some(model.as_str())
                        {
                            "*"
                        } else {
                            " "
                        };
                        format!(
                            "{marker} {}. {model} (openai-compatible:deepseek/{model})",
                            idx + 1
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                current_model_presets(provider_idx)
                    .iter()
                    .enumerate()
                    .map(|(idx, (label, provider, model))| {
                        let marker = if app.config.provider == *provider
                            && app.config.model.as_deref() == Some(*model)
                        {
                            "*"
                        } else {
                            " "
                        };
                        let shortcut = match family {
                            ProviderFamily::Codex
                            | ProviderFamily::DeepSeek
                            | ProviderFamily::OpenAiCompatible
                            | ProviderFamily::Gemini
                            | ProviderFamily::CandleLocal
                            | ProviderFamily::Bedrock => (idx + 1).to_string(),
                            ProviderFamily::Ollama => model.to_string(),
                        };
                        format!("{marker} {shortcut}. {label} ({provider}/{model})")
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            format!("{title}\n{provider_lines}")
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    format!(
        "Current model: {} / {}\n\nAvailable presets:\n{}\n\nGemma 4 Candle presets are marked experimental.\n\nUse /model to open the interactive provider and model flow.",
        app.config.provider,
        app.current_model_label(),
        lines
    )
}

pub fn api_key_status(config: &RaraConfig) -> &'static str {
    if !provider_requires_api_key(&config.provider) {
        "not-required"
    } else if config.has_api_key() {
        "configured"
    } else {
        "missing"
    }
}

pub fn is_local_provider(provider: &str) -> bool {
    matches!(
        provider,
        "local" | "local-candle" | "gemma4" | "qwen3" | "qwn3"
    )
}

#[cfg(test)]
mod tests {
    use super::format_unix_timestamp_utc;

    #[test]
    fn format_unix_timestamp_utc_renders_readable_utc_time() {
        assert_eq!(format_unix_timestamp_utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(
            format_unix_timestamp_utc(1_777_584_000),
            "2026-04-30 21:20:00 UTC"
        );
    }
}
