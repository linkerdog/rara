use std::time::Instant;

use ratatui::{layout::Rect, style::Color, text::Line};
use tempfile::tempdir;
use tokio::sync::mpsc;

use super::super::view_builder::{
    activity_status_line, build_bottom_pane_view, footer_summary_text, should_show_spinner,
};
use crate::config::ConfigManager;
use crate::tui::message_role::MessageRole;
use crate::tui::render::bottom_pane::composer::{
    composer_hint, composer_hint_line, desired_composer_height, wrapped_text_cursor_position,
    wrapped_text_rows,
};
use crate::tui::state::NoticeLevel;
use crate::tui::state::{
    InteractionKind, PendingInteractionSnapshot, RunningTask, RuntimePhase, RuntimeSnapshot,
    TaskCompletion, TaskKind, TuiApp,
};
use crate::tui::theme::STATUS_WARNING;

#[test]
fn footer_summary_text_reports_permission_and_approval_when_idle() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.snapshot = RuntimeSnapshot {
        estimated_history_tokens: 1234,
        context_window_tokens: Some(32768),
        ..RuntimeSnapshot::default()
    }
    .into();

    let rendered = footer_summary_text(&app);
    assert_eq!(rendered, "perm=custom approval=suggestion");
    assert!(!rendered.contains("tokens="));
    assert!(!rendered.contains("ctx~="));
}

#[test]
fn footer_summary_text_shows_tokens_while_busy() {
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
    }
    .into();

    let rendered = footer_summary_text(&app);
    assert_eq!(rendered, "perm=custom approval=suggestion  tokens=2.0k");
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
    }
    .into();

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
    app.push_notice(NoticeLevel::Error, "An unrelated request failed");
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
            created_at_epoch_seconds: None,
        });

    let (label, _, detail) = activity_status_line(&app);
    assert_eq!(label, "Plan Approval");
    assert!(detail.contains("approve"));
    assert!(detail.contains("keep planning"));
    assert!(detail.contains("reject"));
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
            created_at_epoch_seconds: None,
        });

    let hint = composer_hint(&app).to_string();
    assert!(hint.contains("approval required"));
    assert!(hint.contains("up/down select"));
    assert!(hint.contains("enter apply"));
    assert!(hint.contains("1-4 shortcut"));
    assert!(!hint.contains("queued follow-up"));
}

#[test]
fn activity_status_line_renders_warning_notice_in_yellow() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.push_notice(
        NoticeLevel::Warning,
        "Warning: openai-compatible is missing an API key. Use /model to configure the current provider.",
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
        query_control: None,
    });
    app.push_notice(NoticeLevel::Error, "An unrelated request failed");
    assert_eq!(activity_status_line(&app).0, "Working");

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
        query_control: None,
    });

    assert_eq!(composer_hint(&app).to_string(), "Enter queue");

    if let Some(task) = app.bottom_pane.running_task.take() {
        task.handle.abort();
    }
}

#[tokio::test]
async fn review_preparation_shows_cancel_hint_and_spinner() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::LocalCommand;
    let (_sender, receiver) = mpsc::unbounded_channel();
    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::ReviewPreparation,
        receiver,
        handle: tokio::spawn(std::future::pending::<TaskCompletion>()),
        started_at: Instant::now(),
        next_heartbeat_after_secs: 2,
        cancellation_token: None,
        query_control: None,
    });
    assert_eq!(
        composer_hint_line(&app).to_string(),
        "Enter queue  Esc/Ctrl+C cancel"
    );
    let (label, _, _) = activity_status_line(&app);
    assert!(should_show_spinner(&app, label));
    app.bottom_pane.running_task.take().unwrap().handle.abort();
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
fn wrapped_text_cache_keeps_indent_variants_separate() {
    let input = "abcdefghij";
    assert_eq!(
        wrapped_text_rows(input, 6, None, None),
        vec!["abcdef", "ghij"]
    );
    assert_eq!(
        wrapped_text_rows(input, 6, Some("› "), Some("  ")),
        vec!["› abcd", "  efgh", "  ij"]
    );
    assert_eq!(
        wrapped_text_rows(input, 6, Some("› "), None),
        vec!["› abcd", "efghij"]
    );
    assert_eq!(
        wrapped_text_rows(input, 6, None, None),
        vec!["abcdef", "ghij"]
    );
}

#[test]
fn composer_height_counts_the_same_indented_rows_as_rendering() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
    app.bottom_pane.input = "0123456789".into();
    assert_eq!(super::composer_content_line_count(&app, 6), 3);
}

#[test]
fn review_regression_terminal_viewport_includes_bottom_pane_once() {
    let mut tui =
        crate::tui::testing::TuiHarness::new(RuntimeSnapshot::default()).expect("harness");
    tui.app_mut()
        .push_entry(MessageRole::User, "Earlier prompt");
    for input in ["short", "first\nsecond\nthird\nfourth"] {
        tui.app_mut().bottom_pane.input = input.into();
        assert_eq!(
            crate::tui::testing::terminal_emulator::render_app_viewport(tui.app_mut(), 80, 24)
                .height,
            24
        );
    }
}

#[test]
fn composer_height_uses_the_rendered_main_width() {
    use crate::tui::render::bottom_pane::desired_bottom_pane_height;
    use crate::tui::testing::TuiHarness;

    let mut tui = TuiHarness::new(RuntimeSnapshot::default()).expect("harness");
    tui.app_mut()
        .push_entry(MessageRole::User, "Earlier prompt");
    tui.app_mut().bottom_pane.input = "x".repeat(720);
    for (terminal_width, sidebar_visible, main_width) in [
        (80, true, 80_u16),
        (120, true, 120),
        (121, true, 83),
        (160, true, 122_u16),
        (160, false, 160),
    ] {
        tui.app_mut().sidebar_visible = sidebar_visible;
        let rendered_width = crate::tui::pane_geometry::PaneColumns {
            terminal_width,
            sidebar_visible,
        }
        .main_width();
        assert_eq!(rendered_width, main_width);
        let expected_bottom = 720_usize.div_ceil(usize::from(main_width - 2)).max(3) as u16 + 2;
        assert_eq!(
            desired_bottom_pane_height(tui.app(), rendered_width, 40),
            expected_bottom,
            "width={terminal_width}, sidebar={sidebar_visible}"
        );
    }
}

#[test]
fn rendered_resize_updates_the_width_used_by_vertical_navigation() {
    use crate::tui::testing::TuiHarness;

    let mut tui = TuiHarness::new(RuntimeSnapshot::default()).expect("harness");
    for (terminal_width, sidebar_visible, main_width) in
        [(160, true, 122_u16), (160, false, 160), (80, true, 80)]
    {
        let offset = usize::from(main_width - 2) + 7;
        let app = tui.app_mut();
        app.terminal_width = if terminal_width == 80 { 160 } else { 80 };
        app.sidebar_visible = sidebar_visible;
        app.bottom_pane.input = "abcdefghijklmnopqrstuvwxyz".repeat(20);
        app.bottom_pane.input_cursor_offset = Some(offset);
        tui.screen_buffer(terminal_width, 40);
        assert_eq!(tui.app().terminal_width, terminal_width);
        tui.app_mut().move_composer_cursor_up();
        assert_eq!(tui.app().composer_cursor_offset(), 7);
        tui.app_mut().move_composer_cursor_down();
        assert_eq!(tui.app().composer_cursor_offset(), offset);
    }
}

#[test]
fn measured_composer_rows_are_not_rewrapped_at_degenerate_widths() {
    use crate::tui::testing::TuiHarness;

    let mut tui = TuiHarness::new(RuntimeSnapshot::default()).expect("harness");
    tui.app_mut().bottom_pane.input = "ab".into();
    tui.app_mut().bottom_pane.input_cursor_offset = Some(0);
    for width in [1, 2, 3] {
        let (buffer, cursor) = tui.screen_buffer(width, 40);
        let (_, top) = cursor.expect("cursor");
        let expected = wrapped_text_rows("ab", width, Some("› "), Some("  "));
        for (row, text) in expected.iter().take(2).enumerate() {
            let rendered = (0..width)
                .map(|x| buffer[(x, top + row as u16)].symbol())
                .collect::<String>();
            assert_eq!(
                rendered,
                text.chars().take(usize::from(width)).collect::<String>(),
                "width={width}, row={row}"
            );
        }
    }
}

#[test]
fn placeholder_uses_the_measured_layout_and_discards_stale_draft_scroll() {
    use crate::tui::testing::TuiHarness;

    let mut tui = TuiHarness::new(RuntimeSnapshot::default()).expect("harness");
    tui.app_mut().bottom_pane.composer_scroll = 99;
    let (buffer, cursor) = tui.screen_buffer(24, 40);
    let cursor = cursor.expect("composer cursor");
    assert_eq!(cursor.0, 2);
    assert_eq!(tui.app().bottom_pane.composer_scroll, 0);
    let expected = wrapped_text_rows(super::COMPOSER_PLACEHOLDER, 24, Some("› "), Some("  "));
    assert_eq!(expected[0], "› Ask about the repo, re");
    for (row, expected) in expected.iter().take(3).enumerate() {
        let actual = (0..24)
            .map(|x| buffer[(x, cursor.1 + row as u16)].symbol())
            .collect::<String>();
        assert_eq!(actual.trim_end(), expected.trim_end());
    }
}

#[test]
fn composer_vertical_movement_tracks_the_rendered_row_with_or_without_sidebar() {
    use crate::tui::testing::TuiHarness;

    let mut tui = TuiHarness::new(RuntimeSnapshot::default()).expect("harness");
    for (terminal_width, sidebar_visible, main_width) in [
        (80, true, 80),
        (120, true, 120),
        (160, true, 122),
        (160, false, 160),
    ] {
        let content_width = usize::from(main_width - 2);
        let cursor_offset = content_width + 7;
        let app = tui.app_mut();
        app.terminal_width = terminal_width;
        app.sidebar_visible = sidebar_visible;
        app.bottom_pane.input = "abcdefghijklmnopqrstuvwxyz".repeat(20);
        app.bottom_pane.input_cursor_offset = Some(cursor_offset);
        app.bottom_pane.composer_scroll = 0;
        let expected_character = app
            .bottom_pane
            .input
            .chars()
            .nth(cursor_offset)
            .expect("cursor character")
            .to_string();
        let (before_buffer, before_cursor) = tui.screen_buffer(terminal_width, 40);
        let before = before_cursor.expect("composer cursor");
        assert_eq!(before.0, terminal_width - main_width + 9);
        assert_eq!(before_buffer[before].symbol(), expected_character);
        tui.app_mut().move_composer_cursor_up();
        let (up_buffer, up_cursor) = tui.screen_buffer(terminal_width, 40);
        assert_eq!(
            up_cursor,
            Some((before.0, before.1 - 1)),
            "width={terminal_width}, sidebar={sidebar_visible}"
        );
        assert_eq!(tui.app().composer_cursor_offset(), 7);
        assert_eq!(up_buffer[up_cursor.expect("up cursor")].symbol(), "h");
        tui.app_mut().move_composer_cursor_down();
        assert_eq!(tui.screen_with_cursor(terminal_width, 40).1, Some(before));
        assert_eq!(tui.app().composer_cursor_offset(), cursor_offset);
    }
}

#[test]
fn composer_last_column_navigation_preserves_insertion_offsets() {
    use crate::tui::testing::TuiHarness;

    let mut tui = TuiHarness::new(RuntimeSnapshot::default()).expect("harness");
    for terminal_width in [80, 120, 160] {
        let content_width = usize::from(terminal_width - 2);
        for input in [
            "z".repeat(content_width * 2),
            format!(
                "{}\n{}\n",
                "z".repeat(content_width),
                "z".repeat(content_width)
            ),
        ] {
            let second_row_end = if input.contains('\n') {
                content_width * 2 + 1
            } else {
                content_width * 2
            };
            for offset in [second_row_end - 1, second_row_end] {
                if !input.contains('\n') && offset == second_row_end {
                    // A soft-wrapped previous row has no insertion boundary past
                    // its last character. Nearest-column movement still clamps there.
                    continue;
                }
                let app = tui.app_mut();
                app.terminal_width = terminal_width;
                app.sidebar_visible = false;
                app.bottom_pane.input = input.clone();
                app.bottom_pane.input_cursor_offset = Some(offset);
                tui.app_mut().move_composer_cursor_up();
                tui.app_mut().move_composer_cursor_down();
                assert_eq!(
                    tui.app().composer_cursor_offset(),
                    offset,
                    "width={terminal_width}, offset={offset}, newline={}",
                    input.contains('\n')
                );
                tui.app_mut().insert_active_input_char('X');
                assert_eq!(tui.app().bottom_pane.input.chars().nth(offset), Some('X'));
            }
        }
    }
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
        query_control: None,
    });
    app.queue_follow_up_message("first follow-up");
    app.queue_follow_up_message("second follow-up");
    app.queue_follow_up_message("third follow-up");

    let (label, _, detail) = activity_status_line(&app);
    assert_eq!(label, "Working");
    assert!(detail.contains("esc to interrupt"));
    assert!(should_show_spinner(&app, label));

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
fn activity_status_line_shows_completed_prompt_notice_until_expiry() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.push_notice(NoticeLevel::Info, "Prompt finished.");

    let (label, _, detail) = activity_status_line(&app);

    assert_eq!(label, "Ready");
    assert_eq!(detail, "Prompt finished.");
    assert!(app.expire_notice(tokio::time::Instant::now() + std::time::Duration::from_secs(8)));
    assert_eq!(activity_status_line(&app).2, "waiting for input");
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

#[test]
fn blocked_goal_uses_compact_warning_badge() {
    use crate::tui::state::{GoalStatus, RalphGoal};

    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let mut goal = RalphGoal::new("wait for external change".into(), None);
    goal.status = GoalStatus::Blocked;
    app.goal = Some(goal);

    let view = build_bottom_pane_view(&app, 80, 24);

    assert_eq!(view.activity.goal_label, Some(("Blocked", STATUS_WARNING)));
}

#[test]
fn composer_height_respects_40_percent_cap() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");

    app.bottom_pane
        .input
        .push_str("line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10");

    let w = 80;
    let h = desired_composer_height(&app, w, 24);
    assert!(h <= 10, "24-row terminal capped at 10, got {h}");
    let h2 = desired_composer_height(&app, w, 80);
    assert!(h2 <= 32, "80-row terminal capped at 32, got {h2}");
    let h3 = desired_composer_height(&app, w, 5);
    assert_eq!(h3, 3, "tiny terminal floors at 3");
}
