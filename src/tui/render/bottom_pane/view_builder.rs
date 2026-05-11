// Bottom pane view builder — pre-computes structured view data from TuiApp.
use ratatui::style::Color;

use super::super::super::state::{
    ActivePendingInteractionKind, GoalStatus, PendingInteractionSnapshot, RalphGoal, RuntimePhase,
    TaskKind, TuiApp,
};
use super::view::{ActivityView, BottomPaneView, FooterView};
use crate::tui::theme::{
    INTERACTION_SUB_AGENT, STATUS_INFO, STATUS_READY, STATUS_SUCCESS, STATUS_WARNING, TEXT_ACCENT,
};

pub(super) fn build_bottom_pane_view(app: &TuiApp, width: u16, _height: u16) -> BottomPaneView {
    BottomPaneView {
        activity: build_activity_view(app, width),
        footer: build_footer_view(app),
    }
}

fn build_activity_view(app: &TuiApp, _width: u16) -> ActivityView {
    let (label, label_color, detail) = activity_status_line(app);
    let animated = animated_activity_label(app, label);
    let label_already_reflects_planning = matches!(
        app.active_pending_interaction().map(|item| item.kind),
        Some(
            ActivePendingInteractionKind::PlanApproval
                | ActivePendingInteractionKind::PlanningQuestion
        )
    ) || matches!(label, "Planning");
    let plan_badge = app.agent_execution_mode_label() == "plan" && !label_already_reflects_planning;
    let perm_badge = app.permission_mode_label() != "auto";
    let perm_label = app.permission_mode_label();
    let goal_label = app.goal.as_ref().map(|goal| goal_label_text(goal.status));
    let goal_detail = app.goal.as_ref().map(|goal| goal_detail_text(goal));

    ActivityView {
        label: animated,
        label_color,
        detail,
        plan_badge,
        perm_badge,
        perm_label,
        goal_label,
        goal_detail,
    }
}

fn goal_label_text(status: GoalStatus) -> (&'static str, Color) {
    match status {
        GoalStatus::Pursuing => ("pursuing", STATUS_INFO),
        GoalStatus::Paused => ("paused", STATUS_WARNING),
        GoalStatus::Complete => ("done", STATUS_SUCCESS),
        GoalStatus::BudgetLimited => ("budget", STATUS_WARNING),
    }
}

fn goal_detail_text(goal: &RalphGoal) -> String {
    if let Some(budget) = goal.token_budget {
        format!(
            "t{} · {}/{} tokens · {} left",
            goal.turns_completed,
            goal.tokens_used,
            budget,
            goal.remaining_tokens().unwrap_or(0)
        )
    } else {
        format!("t{} · {} tokens", goal.turns_completed, goal.tokens_used)
    }
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

    if app.bottom_pane.has_pending_planning_suggestion() {
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
        .bottom_pane
        .notice
        .as_deref()
        .filter(|value| value.starts_with("Warning:"))
    {
        return ("Warning", STATUS_WARNING, warning.to_string());
    }

    (
        "Ready",
        STATUS_READY,
        app.bottom_pane
            .notice
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
    let Some(task) = app.bottom_pane.running_task.as_ref() else {
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

fn build_footer_view(app: &TuiApp) -> FooterView {
    FooterView {
        text: footer_summary_text(app),
        hide: matches!(
            app.overlay,
            Some(crate::tui::state::Overlay::CommandPalette)
        ),
    }
}

pub(super) fn footer_summary_text(app: &TuiApp) -> String {
    let mut parts: Vec<String> = Vec::new();

    if let Some(hint) = app.repo_context_hint() {
        parts.push(hint);
    }

    if shows_live_task_stats(app) {
        parts.push(format!(
            "tokens={}",
            crate::tui::status_display::format_token_count(app.snapshot.estimated_history_tokens,),
        ));
    }

    if let Some(rate) = crate::tui::format::cache_hit_rate_label(
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
