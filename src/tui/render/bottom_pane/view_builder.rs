// Bottom pane view builder — pre-computes structured view data from TuiApp.
use ratatui::{style::Color, text::Line};

use super::super::super::interaction_text::pending_interaction_hint_text;
use super::super::super::queued_input::{pending_follow_up_hint, queued_follow_up_hint};
use super::super::super::state::{
    ActivePendingInteractionKind, GoalStatus, RuntimePhase, TaskKind, TuiApp,
};
use super::composer::{composer_content_line_count, find_cursor_row_in_wrapped, wrapped_text_rows};
use super::view::*;
use crate::tui::format::cache_hit_rate_label;
use crate::tui::state::Overlay;
use crate::tui::status_display::format_token_count;
use crate::tui::theme::*;

pub(super) fn build_bottom_pane_view(
    app: &mut crate::tui::state::TuiApp,
    width: u16,
    rows: u16,
) -> BottomPaneView {
    let activity = build_activity_view(app);
    let composer = build_composer_view(app, width, rows);
    let footer = build_footer_view(app);
    BottomPaneView {
        activity,
        composer,
        footer,
    }
}

fn build_activity_view(app: &TuiApp) -> ActivityView {
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
    let goal = app.goal.clone();

    ActivityView {
        label: animated,
        label_color,
        detail,
        plan_badge,
        perm_badge,
        perm_label,
        goal,
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
    if !app.is_busy() || app.active_turn.entries.is_empty() {
        return label.to_string();
    }
    if let Some(task) = &app.bottom_pane.running_task {
        let elapsed_ms = task.started_at.elapsed().as_millis();
        let dots = match (elapsed_ms / 500) % 4 {
            0 => "",
            1 => ".",
            2 => "..",
            _ => "...",
        };
        return format!("{label}{dots}");
    }
    label.to_string()
}

fn build_composer_view(app: &mut TuiApp, area_width: u16, area_height: u16) -> ComposerInputView {
    let cursor_offset = app.composer_cursor_offset();
    let input = app.bottom_pane.input.clone();

    let content_rows = composer_content_line_count(app, area_width) as usize;
    let cursor_row =
        find_cursor_row_in_wrapped(&input, cursor_offset, area_width, Some("› "), Some("  "));
    app.maintain_composer_scroll(
        area_width,
        area_height.saturating_sub(1),
        cursor_row,
        content_rows,
    );

    let hint = if matches!(app.overlay, Some(Overlay::CommandPalette)) {
        Line::default()
    } else if input.trim_start().starts_with('/') {
        crate::tui::render::bottom_pane::composer::parse_hint_with_keys(
            "slash command  Enter run  Esc close",
        )
    } else if let Some(pending) = app.active_pending_interaction() {
        let text = pending_interaction_hint_text(pending.kind);
        if text.is_empty() {
            Line::default()
        } else {
            crate::tui::render::bottom_pane::composer::parse_hint_with_keys(text)
        }
    } else if app.has_pending_follow_up_messages() {
        let text = pending_follow_up_hint();
        if text.is_empty() {
            Line::default()
        } else {
            crate::tui::render::bottom_pane::composer::parse_hint_with_keys(text)
        }
    } else if app.has_queued_follow_up_messages() {
        let text = queued_follow_up_hint();
        if text.is_empty() {
            Line::default()
        } else {
            crate::tui::render::bottom_pane::composer::parse_hint_with_keys(text)
        }
    } else if app.is_busy() {
        let text = if app
            .bottom_pane
            .running_task
            .as_ref()
            .is_some_and(|task| matches!(task.kind, TaskKind::Query))
        {
            "Enter queue  Esc/Ctrl+C cancel"
        } else {
            "Enter queue"
        };
        crate::tui::render::bottom_pane::composer::parse_hint_with_keys(text)
    } else if app.has_pending_planning_suggestion() {
        crate::tui::render::bottom_pane::composer::parse_hint_with_keys(
            "planning suggested  1 enter planning mode  2 continue in execute mode",
        )
    } else if app.agent_execution_mode_label() == "plan" {
        crate::tui::render::bottom_pane::composer::parse_hint_with_keys(
            "planning mode  read-only planning; approve to execute",
        )
    } else {
        Line::default()
    };

    ComposerInputView {
        input,
        cursor_offset,
        scroll: app.bottom_pane.composer_scroll,
        hint,
        overlay: app.overlay,
    }
}

fn build_footer_view(app: &TuiApp) -> FooterView {
    let hide = matches!(app.overlay, Some(Overlay::CommandPalette));
    let parts = footer_summary_parts(app);
    FooterView { parts, hide }
}

fn footer_summary_parts(app: &TuiApp) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();

    if let Some(hint) = app.repo_context_hint() {
        parts.push(hint);
    }

    let live = shows_live_task_stats(app);
    if let Some(task) = &app.bottom_pane.running_task
        && live
    {
        let elapsed = task.started_at.elapsed();
        let secs = elapsed.as_secs();
        let mins = secs / 60;
        let secs_remainder = secs % 60;
        let elapsed_str = if mins > 0 {
            format!("elapsed={mins}m{secs_remainder}s")
        } else {
            format!("elapsed={secs}s")
        };
        parts.push(elapsed_str);
    }
    if live && app.snapshot.total_cache_hit_tokens > 0 {
        parts.push(format!(
            "tokens={} (↓{})",
            format_token_count(
                app.snapshot.estimated_history_tokens
                    + app.snapshot.total_cache_hit_tokens as usize
                    + app.snapshot.total_cache_miss_tokens as usize
            ),
            format_token_count(
                (app.snapshot.total_cache_hit_tokens + app.snapshot.total_cache_miss_tokens)
                    as usize
            ),
        ));
    } else if live {
        parts.push(format!(
            "tokens={}",
            format_token_count(app.snapshot.estimated_history_tokens)
        ));
    }

    if let Some(rate) = cache_hit_rate_label(
        app.snapshot.total_cache_hit_tokens,
        app.snapshot.total_cache_miss_tokens,
    ) {
        parts.push(format!("cache_hit={rate}"));
    }

    if !live && app.snapshot.compaction_count > 0 {
        parts.push(format!("compactions={}", app.snapshot.compaction_count));
    }

    parts
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

#[allow(dead_code)]
pub(super) fn footer_summary_text(app: &TuiApp) -> String {
    footer_summary_parts(app).join("  ")
}
