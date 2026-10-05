// Bottom pane view builder — pre-computes structured view data from TuiApp.
use ratatui::style::Color;

use super::super::super::state::{
    ActivePendingInteractionKind, GoalStatus, PendingInteractionSnapshot, RalphGoal, RuntimePhase,
    TaskKind, TuiApp,
};
use super::view::{
    ActivityView, BottomPaneView, FooterView, InteractionAction, InteractionPanelView,
    ShellApprovalView,
};
use crate::tui::theme::{
    INTERACTION_SUB_AGENT, STATUS_INFO, STATUS_READY, STATUS_SUCCESS, STATUS_WARNING, TEXT_ACCENT,
};

const PERMISSION_BADGE_BREAKPOINT: u16 = 80;

pub(super) fn build_bottom_pane_view(app: &TuiApp, width: u16, _height: u16) -> BottomPaneView {
    BottomPaneView {
        activity: build_activity_view(app, width),
        interaction_panel: build_interaction_panel(app),
        footer: build_footer_view(app),
    }
}

fn build_activity_view(app: &TuiApp, width: u16) -> ActivityView {
    let (label, label_color, detail) = activity_status_line(app);
    let spinner = should_show_spinner(app, label);
    let spinner_elapsed = app
        .bottom_pane
        .running_task
        .as_ref()
        .map(|task| task.started_at.elapsed())
        .unwrap_or_default();
    let label_already_reflects_planning = matches!(
        app.active_pending_interaction().map(|item| item.kind),
        Some(
            ActivePendingInteractionKind::PlanApproval
                | ActivePendingInteractionKind::PlanningQuestion
        )
    ) || matches!(label, "Planning");
    let plan_badge = app.agent_execution_mode_label() == "plan" && !label_already_reflects_planning;
    let perm_badge = width < PERMISSION_BADGE_BREAKPOINT && app.permission_mode_label() != "auto";
    let perm_label = app.permission_mode_label();
    let goal_label = app.goal.as_ref().map(|goal| goal_label_text(goal.status));
    let goal_detail = app.goal.as_ref().map(goal_detail_text);

    ActivityView {
        label,
        label_color,
        spinner,
        spinner_elapsed,
        detail: crate::tui::display_sanitize::sanitize_display_line(&detail),
        plan_badge,
        perm_badge,
        perm_label,
        goal_label,
        goal_detail,
    }
}

fn goal_label_text(status: GoalStatus) -> (&'static str, Color) {
    let color = match status {
        GoalStatus::Pursuing => STATUS_INFO,
        GoalStatus::Paused | GoalStatus::Blocked | GoalStatus::BudgetLimited => STATUS_WARNING,
        GoalStatus::Complete => STATUS_SUCCESS,
    };
    (crate::tui::goal_ui::status_label(status), color)
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
        format!(
            "{}s · {} tokens",
            goal.time_used_seconds(),
            goal.tokens_used
        )
    }
}

pub(super) fn activity_status_line(app: &TuiApp) -> (&'static str, Color, String) {
    if app.pending_restore.is_some() {
        return (
            "Resuming",
            STATUS_INFO,
            "Loading saved thread · Esc to cancel".into(),
        );
    }
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
                "approve, keep planning, or reject the proposed plan".to_string()
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

pub(super) fn should_show_spinner(app: &TuiApp, label: &str) -> bool {
    if label.is_empty() {
        return false;
    }
    let Some(task) = app.bottom_pane.running_task.as_ref() else {
        return false;
    };
    matches!(
        task.kind,
        TaskKind::Query | TaskKind::ReviewPreparation | TaskKind::Rebuild
    )
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
    if let Some(key) = app.quit_shortcut.key() {
        let key = match key {
            crate::tui::state::QuitShortcutKey::CtrlC => "Ctrl-C",
            crate::tui::state::QuitShortcutKey::CtrlD => "Ctrl-D",
        };
        return format!("Press {key} again to quit");
    }
    let mut parts: Vec<String> = Vec::new();

    if let Some(hint) = app.repo_context_hint() {
        parts.push(hint);
    }

    parts.push(footer_permission_status(app));

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

fn footer_permission_status(app: &TuiApp) -> String {
    let mut status = format!(
        "perm={} approval={}",
        app.permission_mode_label(),
        app.bash_approval_mode_label()
    );
    if let Some(mode) = app.pending_permission_mode {
        status.push_str(&format!(" pending={} (after task)", mode.label()));
    }
    status
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

pub(super) fn build_interaction_panel(app: &TuiApp) -> Option<InteractionPanelView> {
    let pending = app.active_pending_interaction()?;

    match pending.kind {
        ActivePendingInteractionKind::ShellApproval => {
            let approval = pending._snapshot.approval.as_ref()?;
            let cwd = approval
                .payload
                .cwd
                .as_deref()
                .filter(|cwd| !cwd.trim().is_empty())
                .unwrap_or(".")
                .to_owned();
            Some(InteractionPanelView {
                title: "Permission Required",
                color: STATUS_WARNING,
                detail: format!("{}\ncwd: {cwd}", approval.command),
                shell_approval: Some(ShellApprovalView {
                    tool_use_id: approval.tool_use_id.clone(),
                    cwd,
                }),
                actions: vec![
                    InteractionAction {
                        key: "1",
                        label: "Allow once",
                    },
                    InteractionAction {
                        key: "2",
                        label: "Allow prefix",
                    },
                    InteractionAction {
                        key: "3",
                        label: "Allow session",
                    },
                    InteractionAction {
                        key: "4",
                        label: "Reject",
                    },
                ],
                selected: app.approval_picker_idx,
            })
        }
        ActivePendingInteractionKind::PlanApproval => Some(InteractionPanelView {
            title: "Plan Approval",
            color: TEXT_ACCENT,
            detail: String::new(),
            shell_approval: None,
            actions: vec![
                InteractionAction {
                    key: "1",
                    label: "approve",
                },
                InteractionAction {
                    key: "2",
                    label: "keep planning",
                },
                InteractionAction {
                    key: "3",
                    label: "reject",
                },
            ],
            selected: app.approval_picker_idx,
        }),
        ActivePendingInteractionKind::PlanningQuestion => Some(InteractionPanelView {
            title: "Planning Question",
            color: TEXT_ACCENT,
            detail: app
                .pending_request_input()
                .map(|interaction| interaction.title.clone())
                .unwrap_or_default(),
            shell_approval: None,
            actions: vec![
                InteractionAction {
                    key: "Enter",
                    label: "Continue Plan",
                },
                InteractionAction {
                    key: "I",
                    label: "Start Implementation",
                },
            ],
            selected: app.approval_picker_idx,
        }),
        _ => None,
    }
}
