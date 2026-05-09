use std::time::Instant;

use ratatui::{layout::Rect, style::Color, text::Line};
use tempfile::tempdir;
use tokio::sync::mpsc;

use crate::config::ConfigManager;
use crate::tui::render::bottom_pane::composer::{
    composer_hint, composer_hint_line, wrapped_text_cursor_position, wrapped_text_rows,
};
use crate::tui::render::bottom_pane::status::{
    activity_status_line, animated_activity_label, footer_summary_text,
};
use crate::tui::state::{
    InteractionKind, PendingInteractionSnapshot, RunningTask, RuntimePhase, RuntimeSnapshot,
    TaskCompletion, TaskKind, TuiApp,
};
use crate::tui::theme::STATUS_WARNING;

#[test]
fn footer_summary_text_is_empty_when_idle_and_no_repo_context() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.snapshot = RuntimeSnapshot {
        estimated_history_tokens: 1234,
        context_window_tokens: Some(32768),
        ..RuntimeSnapshot::default()
    };

    let rendered = footer_summary_text(&app);
    assert_eq!(rendered, "");
}

#[test]
fn footer_summary_text_shows_tokens_only_while_busy() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::ProcessingResponse;
    app.snapshot = RuntimeSnapshot {
        estimated_history_tokens: 2048,
        context_window_tokens: Some(32768),
        total_input_tokens: 111,
        total_output_tokens: 22,
        ..RuntimeSnapshot::default()
    };

    let rendered = footer_summary_text(&app);
    assert_eq!(rendered, "tokens=111 in / 22 out");
    assert!(!rendered.contains("history="));
    assert!(!rendered.contains("local="));
    assert!(!rendered.contains("key="));
}

#[test]
fn footer_summary_text_shows_cache_hit_rate_when_usage_has_cache_tokens() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::ProcessingResponse;
    app.snapshot = RuntimeSnapshot {
        estimated_history_tokens: 2048,
        context_window_tokens: Some(32768),
        total_input_tokens: 111,
        total_output_tokens: 22,
        total_cache_hit_tokens: 80,
        total_cache_miss_tokens: 20,
        ..RuntimeSnapshot::default()
    };

    let rendered = footer_summary_text(&app);
    assert!(rendered.contains("cache_hit=80.0%"));
}

#[test]
fn activity_status_line_prefers_pending_interactions() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.snapshot
        .pending_interactions
        .push(PendingInteractionSnapshot {
            kind: InteractionKind::PlanApproval,
            title: "Approve plan".into(),
            summary: "ready".into(),
            options: Vec::new(),
            note: None,
            approval: None,
            source: None,
        });

    let (label, _, detail) = activity_status_line(&app);
    assert_eq!(label, "Plan Approval");
    assert!(detail.contains("start implementation"));
    assert!(detail.contains("continue planning"));
}

#[test]
fn pending_interaction_hint_takes_priority_over_queued_follow_up() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.queue_follow_up_message("then review the diff");
    app.snapshot
        .pending_interactions
        .push(PendingInteractionSnapshot {
            kind: InteractionKind::Approval,
            title: "Shell Approval".into(),
            summary: "git diff origin/main -- src/context/assembler.rs".into(),
            options: Vec::new(),
            note: None,
            approval: None,
            source: None,
        });

    let hint = composer_hint(&app).to_string();
    assert!(hint.contains("1 allow once"));
    assert!(!hint.contains("queued follow-up"));
}

#[test]
fn activity_status_line_renders_warning_notice_in_yellow() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.bottom_pane.notice = Some(
        "Warning: openai-compatible is missing an API key. Use /model to configure the current provider."
            .into(),
    );

    let (label, color, detail) = activity_status_line(&app);
    assert_eq!(label, "Warning");
    assert_eq!(color, STATUS_WARNING);
    assert!(detail.contains("missing an API key"));
}

#[test]
fn queued_follow_up_hint_stays_compact_while_busy() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::ProcessingResponse;
    app.begin_running_turn();
    app.queue_follow_up_message_after_next_tool_boundary("follow-up");

    assert_eq!(composer_hint(&app).to_string(), "queued: after tool");
}

#[tokio::test]
async fn busy_composer_hint_keeps_only_action_keys() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::ProcessingResponse;
    let (_sender, receiver) = mpsc::unbounded_channel();
    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::Query,
        receiver,
        handle: tokio::spawn(std::future::pending::<TaskCompletion>()),
        started_at: Instant::now(),
        next_heartbeat_after_secs: 2,
        cancellation_token: None,
        cancellation_requested: false,
    });

    assert_eq!(
        composer_hint(&app).to_string(),
        "Enter queue  Esc/Ctrl+C cancel"
    );

    if let Some(task) = app.bottom_pane.running_task.take() {
        task.handle.abort();
    }
}

#[tokio::test]
async fn busy_composer_hint_hides_cancel_for_non_query_tasks() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::ProcessingResponse;
    let (_sender, receiver) = mpsc::unbounded_channel();
    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::Compact,
        receiver,
        handle: tokio::spawn(std::future::pending::<TaskCompletion>()),
        started_at: Instant::now(),
        next_heartbeat_after_secs: 2,
        cancellation_token: None,
        cancellation_requested: false,
    });

    assert_eq!(composer_hint(&app).to_string(), "Enter queue");

    if let Some(task) = app.bottom_pane.running_task.take() {
        task.handle.abort();
    }
}

#[test]
fn composer_hint_line_excludes_repo_context() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.repo_slug = Some("hawkingrei/rara".into());
    app.snapshot.branch = "feat/test".into();

    let rendered = composer_hint_line(&app).to_string();
    assert!(!rendered.contains("repo:"));
    assert!(!rendered.contains("branch:"));
    assert!(!rendered.contains("Enter submit"));
}

#[test]
fn composer_hint_line_hides_slash_hint_while_palette_is_open() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.bottom_pane.input = "/".into();
    app.overlay = Some(crate::tui::state::Overlay::CommandPalette);

    let rendered = composer_hint_line(&app).to_string();
    assert!(!rendered.contains("slash command"));
}

#[test]
fn footer_summary_text_includes_repo_context_when_available() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.repo_slug = Some("hawkingrei/rara".into());
    app.current_pr_url = Some("https://github.com/hawkingrei/rara/pull/46".into());
    app.snapshot.branch = "feat/test".into();
    app.snapshot.estimated_history_tokens = 1234;
    app.snapshot.context_window_tokens = Some(32768);

    let rendered = footer_summary_text(&app);
    assert!(rendered.contains("repo: hawkingrei/rara"));
    assert!(rendered.contains("branch: feat/test"));
    assert!(rendered.contains("PR: https://github.com/hawkingrei/rara/pull/46"));
    assert!(!rendered.contains("ctx~="));
}

#[test]
fn wrapped_text_rows_preserve_space_only_and_blank_lines() {
    let rows = wrapped_text_rows(" \n\n  ", 12, Some("› "), Some("  "));

    assert_eq!(rows, vec!["›  ", "  ", "    "]);
}

#[test]
fn wrapped_text_cursor_tracks_trailing_blank_composer_line() {
    let area = Rect {
        x: 4,
        y: 2,
        width: 12,
        height: 6,
    };

    let cursor = wrapped_text_cursor_position(
        "line one\n",
        "line one\n".chars().count(),
        area,
        Some("› "),
        Some("  "),
    );
    assert_eq!(cursor, (6, 3));
}

#[test]
fn wrapped_text_rows_treat_tabs_as_fixed_width_columns() {
    let rows = wrapped_text_rows("\t12345", 8, Some("› "), Some("  "));

    assert_eq!(rows, vec!["› \t12", "  345"]);
}

#[test]
fn wrapped_text_cursor_treats_tabs_as_fixed_width_columns() {
    let area = Rect {
        x: 0,
        y: 0,
        width: 8,
        height: 4,
    };

    let cursor = wrapped_text_cursor_position(
        "\t12345",
        "\t12345".chars().count(),
        area,
        Some("› "),
        Some("  "),
    );
    assert_eq!(cursor, (5, 1));
}

#[test]
fn wrapped_text_cursor_tracks_space_only_composer_input() {
    let area = Rect {
        x: 0,
        y: 0,
        width: 12,
        height: 4,
    };

    let cursor =
        wrapped_text_cursor_position("   ", "   ".chars().count(), area, Some("› "), Some("  "));
    assert_eq!(cursor, (5, 0));
}

#[test]
fn wrapped_text_cursor_can_point_into_the_middle_of_input() {
    let area = Rect {
        x: 0,
        y: 0,
        width: 12,
        height: 4,
    };

    let cursor = wrapped_text_cursor_position("hello world", 5, area, Some("› "), Some("  "));
    assert_eq!(cursor, (7, 0));
}

#[tokio::test]
async fn activity_status_line_hides_busy_progress_from_composer_bar() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::ProcessingResponse;
    let (_sender, receiver) = mpsc::unbounded_channel();
    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::Query,
        receiver,
        handle: tokio::spawn(std::future::pending::<TaskCompletion>()),
        started_at: Instant::now(),
        next_heartbeat_after_secs: 2,
        cancellation_token: None,
        cancellation_requested: false,
    });
    app.queue_follow_up_message("first follow-up");
    app.queue_follow_up_message("second follow-up");
    app.queue_follow_up_message("third follow-up");

    let (label, _, detail) = activity_status_line(&app);
    assert_eq!(label, "Working");
    assert!(detail.contains("esc to interrupt"));
    assert!(animated_activity_label(&app, label).starts_with("Working"));

    if let Some(task) = app.bottom_pane.running_task.take() {
        task.handle.abort();
    }
}

#[test]
fn composer_hint_shows_compact_queued_follow_up_when_idle() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.queue_follow_up_message("first hint");
    app.queue_follow_up_message("second hint");

    assert_eq!(app.queued_follow_up_count(), 2);
    assert_eq!(composer_hint(&app).to_string(), "queued: after turn");
}

#[test]
fn activity_status_line_hides_completed_prompt_notice() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.bottom_pane.notice = Some("Prompt finished.".into());

    let (label, _, detail) = activity_status_line(&app);

    assert_eq!(label, "Ready");
    assert_eq!(detail, "waiting for input");
}

#[test]
fn goal_is_none_by_default() {
    let temp = tempdir().unwrap();
    let app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    assert!(app.goal.is_none());
}

#[test]
fn setting_goal_preserves_activity_status_label() {
    use crate::tui::state::RalphGoal;

    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.goal = Some(RalphGoal::new("fix the build".into(), None));

    // Goal rendering is in render_activity_bar (badge), not in activity_status_line.
    let (label, _, _) = activity_status_line(&app);
    assert_eq!(label, "Ready");
}
