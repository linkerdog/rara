// Claude Code-style /context display — visual budget bar, clean sections, percentages.
//
// Each line is a ratatui Line with Span-styled values so colors
// actually render in the TUI, not just plain text.
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::tui::format::cache_hit_rate_label;
use crate::tui::state::TuiApp;
use crate::tui::status_display::format_token_count;
use crate::tui::theme::{
    BUDGET_ACTIVE, BUDGET_FREE, BUDGET_HISTORY, BUDGET_MEMORY, BUDGET_OUTPUT, BUDGET_SYSTEM,
    BUDGET_WORKSPACE, STATUS_INFO, STATUS_SUCCESS, TEXT_ACCENT, TEXT_MUTED, TEXT_SECONDARY,
};

pub(crate) fn render_context_lines(app: &TuiApp, available_width: u16) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let snap = &app.snapshot;

    let window = snap.context_window_tokens;
    let used = snap
        .stable_instructions_budget
        .saturating_add(snap.workspace_prompt_budget)
        .saturating_add(snap.active_turn_budget)
        .saturating_add(snap.compacted_history_budget)
        .saturating_add(snap.retrieved_memory_budget)
        .saturating_add(snap.reserved_output_tokens);

    // ── Context Usage ──
    section_header(&mut lines, "Context Usage");
    let routing = app.model_routing_view();
    kv(&mut lines, "model", &routing.main_model, Color::LightBlue);
    kv(
        &mut lines,
        "auxiliary",
        &format!("{} ({})", routing.auxiliary_model, routing.auxiliary_route),
        if routing.auxiliary_uses_main_model {
            TEXT_MUTED
        } else {
            Color::LightBlue
        },
    );
    kv(
        &mut lines,
        "window",
        &format!(
            "{} tokens",
            window
                .map(format_token_count)
                .unwrap_or_else(|| "unknown".to_string())
        ),
        TEXT_SECONDARY,
    );

    // Visual budget bar
    let bar_width = (available_width.saturating_sub(6)).max(20) as usize;
    let bar_line = budget_bar(app, bar_width, used);
    lines.push(bar_line);

    // Usage summary with color-coded percentage
    let used_str = format_token_count(used);
    let pct_value = window
        .filter(|w| *w > 0)
        .map(|w| used as f64 * 100.0 / w as f64)
        .unwrap_or(0.0);
    let pct_color = if pct_value > 80.0 {
        Color::Red
    } else if pct_value > 50.0 {
        Color::Yellow
    } else {
        STATUS_SUCCESS
    };
    let pct_str = format!("{:.1}%", pct_value);
    let window_str = window
        .map(format_token_count)
        .unwrap_or_else(|| "?".to_string());
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            used_str,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" / {window_str}  "),
            Style::default().fg(TEXT_MUTED),
        ),
        Span::styled(
            pct_str,
            Style::default().fg(pct_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" used", Style::default().fg(TEXT_MUTED)),
    ]));
    section_spacer(&mut lines);

    // ── Budget Breakdown ──
    section_header(&mut lines, "Budget Breakdown");
    budget_row(
        &mut lines,
        "System prompt",
        snap.stable_instructions_budget,
        BUDGET_SYSTEM,
        window,
    );
    budget_row(
        &mut lines,
        "Workspace",
        snap.workspace_prompt_budget,
        BUDGET_WORKSPACE,
        window,
    );
    budget_row(
        &mut lines,
        "Active turn",
        snap.active_turn_budget,
        BUDGET_ACTIVE,
        window,
    );
    budget_row(
        &mut lines,
        "History",
        snap.compacted_history_budget,
        BUDGET_HISTORY,
        window,
    );
    budget_row(
        &mut lines,
        "Memory",
        snap.retrieved_memory_budget,
        BUDGET_MEMORY,
        window,
    );
    budget_row(
        &mut lines,
        "Output reserve",
        snap.reserved_output_tokens,
        BUDGET_OUTPUT,
        window,
    );
    if let Some(free) = snap.remaining_input_budget {
        budget_row(&mut lines, "Free", free, BUDGET_FREE, window);
    }
    section_spacer(&mut lines);

    // ── Session ──
    section_header(&mut lines, "Session");
    kv(&mut lines, "cwd", &home_path(&snap.cwd), TEXT_SECONDARY);
    kv(&mut lines, "branch", &snap.branch, TEXT_SECONDARY);
    kv(&mut lines, "session", &snap.session_id, TEXT_MUTED);
    kv(
        &mut lines,
        "history",
        &format!(
            "{} msgs  {} entries",
            snap.history_len,
            app.transcript_entry_count()
        ),
        TEXT_SECONDARY,
    );
    section_spacer(&mut lines);

    // ── Observability ──
    section_header(&mut lines, "Observability");
    // Cache
    kv(
        &mut lines,
        "cache",
        &format!(
            "hit={} miss={} rate={}",
            format_token_count(snap.total_cache_hit_tokens as usize),
            format_token_count(snap.total_cache_miss_tokens as usize),
            cache_hit_rate_label(snap.total_cache_hit_tokens, snap.total_cache_miss_tokens)
                .unwrap_or_else(|| "-".to_string())
        ),
        TEXT_SECONDARY,
    );

    // Microcompact
    if snap.context_observability.microcompact.enabled {
        let mc = &snap.context_observability.microcompact;
        let status = if mc.cache_edit_applied {
            "Applied (provider cache-edit)"
        } else if mc.cache_edit_eligible {
            "Eligible (waiting for provider)"
        } else {
            "Enabled (baseline projection)"
        };
        kv(
            &mut lines,
            "microcompact",
            status,
            if mc.cache_edit_applied {
                STATUS_SUCCESS
            } else {
                TEXT_SECONDARY
            },
        );
        kv(
            &mut lines,
            "projection",
            &format!(
                "cleared={} kept={} saved={}",
                mc.cleared_results,
                mc.kept_results,
                format_char_count(mc.saved_chars)
            ),
            TEXT_MUTED,
        );
    }

    // Retrieval
    let ret = &snap.context_observability.retrieval;
    if ret.candidate_count > 0 {
        kv(
            &mut lines,
            "retrieval",
            &format!(
                "{} providers, {} selected / {} available",
                ret.provider_count, ret.selected_count, ret.available_count
            ),
            TEXT_SECONDARY,
        );
    }
    section_spacer(&mut lines);

    // ── Compaction ──
    if snap.compaction_count > 0 {
        section_header(&mut lines, "Compaction");
        kv(
            &mut lines,
            "estimated",
            &format!(
                "{} tokens",
                format_token_count(snap.estimated_history_tokens)
            ),
            TEXT_SECONDARY,
        );
        kv(
            &mut lines,
            "threshold",
            &format!(
                "{} tokens",
                format_token_count(snap.compact_threshold_tokens)
            ),
            TEXT_SECONDARY,
        );
        kv(
            &mut lines,
            "count",
            &snap.compaction_count.to_string(),
            TEXT_SECONDARY,
        );
        if let (Some(before), Some(after)) = (
            snap.last_compaction_before_tokens,
            snap.last_compaction_after_tokens,
        ) {
            kv(
                &mut lines,
                "last",
                &format!(
                    "{} → {} tokens",
                    format_token_count(before),
                    format_token_count(after)
                ),
                TEXT_MUTED,
            );
        }
        if let Some(version) = snap.last_compaction_boundary_version {
            let before = snap
                .last_compaction_boundary_before_tokens
                .map(format_token_count)
                .unwrap_or_else(|| "-".to_string());
            let files = snap
                .last_compaction_boundary_recent_file_count
                .map(|count| count.to_string())
                .unwrap_or_else(|| "-".to_string());
            kv(
                &mut lines,
                "boundary",
                &format!("v{version} before={before} recent_files={files}"),
                TEXT_MUTED,
            );
        }
        if !snap.compaction_source_entries.is_empty() {
            kv(
                &mut lines,
                "sources",
                &snap
                    .compaction_source_entries
                    .iter()
                    .map(|entry| entry.source_descriptor.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                TEXT_SECONDARY,
            );
        }
        section_spacer(&mut lines);
    }

    // ── Active Turn ──
    section_header(&mut lines, "Active Turn");
    kv(
        &mut lines,
        "mode",
        app.agent_execution_mode_label(),
        Color::LightBlue,
    );
    if snap.plan_steps.is_empty() {
        kv(&mut lines, "plan", "no active plan steps", TEXT_MUTED);
    } else {
        for (idx, (status, step)) in snap.plan_steps.iter().enumerate() {
            let color = match status.as_str() {
                "pending" => TEXT_SECONDARY,
                "in_progress" => STATUS_INFO,
                "completed" => STATUS_SUCCESS,
                _ => TEXT_MUTED,
            };
            kv(
                &mut lines,
                &format!("step {idx}"),
                &format!("[{status}] {step}"),
                color,
            );
        }
    }
    if !snap.pending_interactions.is_empty() {
        kv(
            &mut lines,
            "pending",
            &format!("{} interaction(s)", snap.pending_interactions.len()),
            Color::Yellow,
        );
    }
    // ── Assembly ──
    render_assembly_layer(
        &mut lines,
        app,
        "stable_instructions",
        "Stable Instructions",
    );
    render_assembly_layer(
        &mut lines,
        app,
        "workspace_prompt_sources",
        "Workspace Prompt Sources",
    );
    render_assembly_layer(
        &mut lines,
        app,
        "active_memory_inputs",
        "Active Memory Inputs",
    );
    render_assembly_layer(&mut lines, app, "compacted_history", "Compacted History");
    render_assembly_layer(&mut lines, app, "active_turn_state", "Active Turn State");
    render_assembly_layer(&mut lines, app, "retrieval_ready", "Retrieval-ready");

    lines
}

fn render_assembly_layer(lines: &mut Vec<Line<'static>>, app: &TuiApp, layer: &str, title: &str) {
    let entries: Vec<&crate::context::ContextAssemblyEntry> = app
        .snapshot
        .assembly_entries
        .iter()
        .filter(|e| e.layer == layer)
        .collect();
    if entries.is_empty() {
        return;
    }
    section_header(lines, title);
    for entry in entries {
        let tokens = entry
            .budget_impact_tokens
            .map(format_token_count)
            .unwrap_or_else(|| "-".to_string());
        let path = entry
            .source_path
            .as_deref()
            .filter(|p| !p.is_empty())
            .unwrap_or("-");
        kv(
            lines,
            &entry.kind,
            &format!("{}  {} tokens", path, tokens),
            TEXT_SECONDARY,
        );
    }
    section_spacer(lines);
}

// ── budget bar ──

fn budget_bar(app: &TuiApp, width: usize, used: usize) -> Line<'static> {
    let snap = &app.snapshot;
    let total = snap.context_window_tokens.unwrap_or(1).max(1);

    let used_width = ((used as f64 / total as f64) * width as f64).round() as usize;
    let used_width = used_width.min(width);
    let free_width = width.saturating_sub(used_width);

    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(Span::raw("  "));
    if used_width > 0 {
        spans.push(Span::styled(
            "░".repeat(used_width),
            Style::default().fg(Color::White),
        ));
    }
    if free_width > 0 {
        spans.push(Span::styled(
            "□".repeat(free_width),
            Style::default().fg(BUDGET_FREE),
        ));
    }
    Line::from(spans)
}

fn budget_row(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    tokens: usize,
    color: Color,
    window: Option<usize>,
) {
    let pct = window
        .filter(|w| *w > 0)
        .map(|w| format!(" ({:.2}%)", tokens as f64 * 100.0 / w as f64))
        .unwrap_or_default();
    let value = format!("{}{}", format_token_count(tokens), pct);
    kv(lines, label, &value, color);
}

// ── helpers ──

fn section_header(lines: &mut Vec<Line<'static>>, title: &str) {
    lines.push(Line::from(Span::styled(
        title.to_string(),
        Style::default()
            .fg(TEXT_ACCENT)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));
}

fn section_spacer(lines: &mut Vec<Line<'static>>) {
    lines.push(Line::from(""));
}

fn kv(lines: &mut Vec<Line<'static>>, key: &str, value: &str, value_color: Color) {
    let key_span = Span::styled(format!("  {key:<14} "), Style::default().fg(TEXT_SECONDARY));
    let value_span = Span::styled(value.to_string(), Style::default().fg(value_color));
    lines.push(Line::from(vec![key_span, value_span]));
}

fn format_char_count(chars: usize) -> String {
    if chars >= 1_000_000 {
        format!("{:.1}M chars", chars as f64 / 1_000_000.0)
    } else if chars >= 1_000 {
        format!("{:.1}k chars", chars as f64 / 1_000.0)
    } else {
        format!("{chars} chars")
    }
}

fn home_path(cwd: &str) -> String {
    if let Ok(home) = std::env::var("HOME")
        && let Some(stripped) = cwd.strip_prefix(&home)
    {
        return format!("~{}", stripped);
    }
    cwd.to_string()
}
