// Bottom pane status — activity bar and footer rendering.
use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::super::super::custom_terminal::Frame;
use super::super::super::interaction_text::pending_interaction_hint_text;
use super::super::super::queued_input::{pending_follow_up_hint, queued_follow_up_hint};
use super::super::super::state::{
    ActivePendingInteractionKind, GoalStatus, RuntimePhase, TaskKind, TuiApp,
};
use super::badge;
use super::bottom_pane_style;
use crate::tui::format::cache_hit_rate_label;
use crate::tui::theme::*;

pub(super) fn render_activity_bar(f: &mut Frame, app: &TuiApp, area: Rect) {
    let (label, color, detail) = activity_status_line(app);
    let mut spans = vec![Span::styled(
        animated_activity_label(app, label),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )];
    let label_already_reflects_planning = matches!(
        app.active_pending_interaction().map(|item| item.kind),
        Some(
            ActivePendingInteractionKind::PlanApproval
                | ActivePendingInteractionKind::PlanningQuestion
        )
    ) || matches!(label, "Planning");

    if app.agent_execution_mode_label() == "plan" && !label_already_reflects_planning {
        spans.push(Span::raw("  "));
        spans.push(badge("mode", "plan", TEXT_ACCENT));
    }
    if app.permission_mode_label() != "auto" {
        spans.push(Span::raw("  "));
        spans.push(badge("perm", app.permission_mode_label(), STATUS_INFO));
    }
    if let Some(goal) = app.goal.as_ref() {
        spans.push(Span::raw("  "));
        let (goal_label, goal_color) = match goal.status {
            GoalStatus::Pursuing => ("pursuing", STATUS_INFO),
            GoalStatus::Paused => ("paused", STATUS_WARNING),
            GoalStatus::Complete => ("done", STATUS_SUCCESS),
            GoalStatus::BudgetLimited => ("budget", STATUS_WARNING),
        };
        spans.push(badge("goal", goal_label, goal_color));
        let goal_detail = if let Some(budget) = goal.token_budget {
            format!(
                "t{} · {}/{} tokens · {} left",
                goal.turns_completed,
                goal.tokens_used,
                budget,
                goal.remaining_tokens().unwrap_or(0)
            )
        } else {
            format!("t{} · {} tokens", goal.turns_completed, goal.tokens_used)
        };
        spans.push(Span::raw(" "));
        spans.push(Span::styled(goal_detail, Style::default().fg(TEXT_MUTED)));
    }
    if !detail.is_empty() {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(detail, Style::default().fg(TEXT_SECONDARY)));
    }
    let status = Paragraph::new(Line::from(spans)).style(bottom_pane_style());
    f.render_widget(status, area);
}

pub(super) fn activity_status_line(app: &TuiApp) -> (&'static str, Color, String) {
    if matches!(app.runtime_phase, RuntimePhase::RebuildingBackend) {
        return (
            "Downloading",
            STATUS_INFO,
            app.runtime_phase_detail
                .as_deref()
                .unwrap_or("preparing backend")
                .to_string(),
        );
    }

    if let Some(pending) = app.active_pending_interaction() {
        let (label, color) = match pending.kind {
            ActivePendingInteractionKind::PlanApproval => ("Plan Approval", TEXT_ACCENT),
            ActivePendingInteractionKind::ShellApproval => ("Shell Approval", STATUS_WARNING),
            ActivePendingInteractionKind::PlanningQuestion => ("Planning Question", TEXT_ACCENT),
            ActivePendingInteractionKind::ExplorationQuestion => {
                ("Exploration Question", STATUS_WARNING)
            }
            ActivePendingInteractionKind::SubAgentQuestion => {
                ("Sub-agent Question", INTERACTION_SUB_AGENT)
            }
            ActivePendingInteractionKind::RequestInput => ("Request Input", STATUS_SUCCESS),
        };
        let detail = match pending.kind {
            ActivePendingInteractionKind::PlanApproval => {
                "choose whether to start implementation or continue planning".to_string()
            }
            ActivePendingInteractionKind::ShellApproval => app
                .pending_command_approval()
                .map(|interaction| interaction.summary.clone())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "review the pending shell command".to_string()),
            ActivePendingInteractionKind::PlanningQuestion
            | ActivePendingInteractionKind::ExplorationQuestion
            | ActivePendingInteractionKind::SubAgentQuestion
            | ActivePendingInteractionKind::RequestInput => app
                .pending_request_input()
                .map(|interaction| interaction.title.clone())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "answer the pending question".to_string()),
        };
        return (label, color, detail);
    }

    if app.has_pending_planning_suggestion() {
        return (
            "Planning Suggested",
            TEXT_ACCENT,
            "enter planning mode first or continue in execute mode".to_string(),
        );
    }

    if app.is_busy() {
        let elapsed = app
            .running_elapsed()
            .map(|d| {
                let secs = d.as_secs();
                if secs < 60 {
                    format!("{}s", secs)
                } else {
                    let mins = secs / 60;
                    let remain_secs = secs % 60;
                    format!("{}m {}s", mins, remain_secs)
                }
            })
            .unwrap_or_else(|| "…".to_string());
        return (
            "Working",
            STATUS_WARNING,
            format!("({} • esc to interrupt)", elapsed),
        );
    }

    if app.agent_execution_mode_label() == "plan" {
        return (
            "Planning",
            TEXT_ACCENT,
            "read-only planning; approve to execute".to_string(),
        );
    }

    if let Some(warning) = app
        .notice
        .as_deref()
        .filter(|value| value.starts_with("Warning:"))
    {
        return ("Warning", STATUS_WARNING, warning.to_string());
    }

    (
        "Ready",
        STATUS_READY,
        app.notice
            .as_deref()
            .filter(|notice| !matches!(*notice, "Prompt finished." | "Planning finished."))
            .unwrap_or("waiting for input")
            .to_string(),
    )
}

pub(super) fn animated_activity_label(app: &TuiApp, label: &str) -> String {
    if label.is_empty() {
        return String::new();
    }
    let Some(task) = app.running_task.as_ref() else {
        return label.to_string();
    };
    if !matches!(task.kind, TaskKind::Query | TaskKind::Rebuild) {
        return label.to_string();
    }

    let dots = match (task.started_at.elapsed().as_millis() / 450) % 3 {
        0 => ".",
        1 => "..",
        _ => "...",
    };
    format!("{label}{dots}")
}

pub(super) fn render_footer(f: &mut Frame, app: &TuiApp, area: Rect) {
    if matches!(
        app.overlay,
        Some(super::super::super::state::Overlay::CommandPalette)
    ) {
        f.render_widget(Paragraph::new("").style(bottom_pane_style()), area);
        return;
    }
    let summary = footer_summary_text(app);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            summary,
            Style::default().fg(TEXT_SECONDARY),
        )))
        .style(bottom_pane_style())
        .alignment(Alignment::Right),
        area,
    );
}

pub(super) fn footer_summary_text(app: &TuiApp) -> String {
    let mut parts: Vec<String> = Vec::new();

    if let Some(hint) = app.repo_context_hint() {
        parts.push(hint);
    }

    if shows_live_task_stats(app) {
        parts.push(format!(
            "tokens={} in / {} out",
            app.snapshot.total_input_tokens, app.snapshot.total_output_tokens,
        ));
    }

    if let Some(rate) = cache_hit_rate_label(
        app.snapshot.total_cache_hit_tokens,
        app.snapshot.total_cache_miss_tokens,
    ) {
        parts.push(format!("cache_hit={rate}"));
    }

    if !shows_live_task_stats(app) && app.snapshot.compaction_count > 0 {
        parts.push(format!("compactions={}", app.snapshot.compaction_count));
    }

    parts.join("  ")
}

fn shows_live_task_stats(app: &TuiApp) -> bool {
    app.is_busy()
        || matches!(
            app.runtime_phase,
            RuntimePhase::SendingPrompt
                | RuntimePhase::ProcessingResponse
                | RuntimePhase::RunningTool
        )
}
