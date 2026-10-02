use std::sync::Arc;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Widget,
};
use serde_json::json;
use tempfile::tempdir;

use super::render::cells::{HistoryCell, LspDiagnosticsCell};
use super::selection::{ScreenPosition, TranscriptSelection};
use super::state::{ApiKeyTarget, Overlay};
use super::state::{RalphGoal, TuiApp};
use super::text_wrap::display_width;
use super::transcript_rows::TranscriptRows;
use crate::config::ConfigManager;

fn app_in(temp: &tempfile::TempDir) -> TuiApp {
    TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app")
}

#[test]
fn diagnostic_rows_obey_unicode_boundary_widths() {
    let payload = json!({
        "file": "\u{754c}\u{754c}\n\u{1b}[31msrc.rs",
        "error": "\u{1b}]0;hidden\u{7}failed\nnext",
        "status": {"diagnostic_count": 6, "servers": [{"running": true}]},
        "diagnostics": (0..6).map(|_| json!({
            "file": "\u{754c}.rs", "line": 0, "column": 0, "severity": "error",
            "code": "a\u{301}\u{200b}",
            "message": "\u{1f469}\u{200d}\u{1f4bb}\n\u{1b}[32m\u{754c}a\u{301}"
        })).collect::<Vec<_>>()
    });
    let cell =
        LspDiagnosticsCell::from_message(&format!("lsp_diagnostics\n{payload}")).expect("cell");
    for width in [0, 1, 2, 3, 4, 8, 16, 25, 80] {
        for line in cell.display_lines(width) {
            let text = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>();
            assert!(!text.chars().any(char::is_control), "{text:?}");
            assert!(
                display_width(&text) <= usize::from(width),
                "width {width}: {text:?}"
            );
        }
    }
}

#[test]
fn composer_navigation_uses_unicode_boundaries() {
    let temp = tempdir().expect("tempdir");
    let mut app = app_in(&temp);
    for cluster in [
        "a\u{301}",
        "\u{1f469}\u{200d}\u{1f4bb}",
        "\u{1f1f8}\u{1f1ec}",
        "\u{1f44d}\u{1f3fd}",
        "\u{754c}",
        "\u{200b}",
    ] {
        app.set_input(format!("x{cluster}y"));
        app.bottom_pane.input_cursor_offset = Some(1 + cluster.chars().count());
        app.move_active_input_cursor_left();
        assert_eq!(app.composer_cursor_offset(), 1, "{cluster:?}");
        app.move_active_input_cursor_right();
        assert_eq!(app.composer_cursor_offset(), 1 + cluster.chars().count());
    }
}

#[test]
fn composer_backspace_uses_unicode_boundaries() {
    let temp = tempdir().expect("tempdir");
    let mut app = app_in(&temp);
    for cluster in [
        "a\u{301}",
        "\u{1f469}\u{200d}\u{1f4bb}",
        "\u{1f1f8}\u{1f1ec}",
    ] {
        app.set_input(format!("x{cluster}"));
        app.backspace_active_input();
        assert_eq!(app.bottom_pane.input, "x", "{cluster:?}");
        assert_eq!(app.composer_cursor_offset(), 1);
    }
}

#[test]
fn composer_delete_uses_unicode_boundaries() {
    let temp = tempdir().expect("tempdir");
    let mut app = app_in(&temp);
    for cluster in [
        "a\u{301}",
        "\u{1f469}\u{200d}\u{1f4bb}",
        "\u{1f1f8}\u{1f1ec}",
    ] {
        app.set_input(format!("x{cluster}y"));
        app.bottom_pane.input_cursor_offset = Some(1);
        app.delete_forward_active_input();
        assert_eq!(app.bottom_pane.input, "xy", "{cluster:?}");
        assert_eq!(app.composer_cursor_offset(), 1);
    }
}

#[test]
fn inserted_joiner_snaps_to_unicode_boundary() {
    let temp = tempdir().expect("tempdir");
    let mut app = app_in(&temp);
    app.set_input("\u{1f469}\u{1f4bb}".to_string());
    app.bottom_pane.input_cursor_offset = Some(1);
    app.insert_active_input_char('\u{200d}');
    assert_eq!(app.composer_cursor_offset(), 3);
}

#[test]
fn paste_burst_joiner_snaps_to_unicode_boundary() {
    let temp = tempdir().expect("tempdir");
    let mut app = app_in(&temp);
    app.set_input("\u{1f469}\u{1f4bb}".to_string());
    app.bottom_pane.input_cursor_offset = Some(1);
    app.bottom_pane.handle_paste_burst_chunk("\u{200d}");
    assert!(app.bottom_pane.flush_paste_burst());
    assert_eq!(app.composer_cursor_offset(), 3);
}

#[test]
fn canonical_rows_copy_only_visible_unicode_boundaries() {
    let source = "\u{301}a\u{200b}b\u{1f469}\u{200d}\u{1f4bb}";
    let expected = "ab\u{1f469}\u{200d}\u{1f4bb}";
    let rows = TranscriptRows::from_visual_lines(vec![Line::from(source)]);
    let row = rows.get(0).expect("row");
    assert_eq!(row.text, expected);
    let area = Rect::new(0, 0, 8, 1);
    let mut buffer = Buffer::empty(area);
    row.line.clone().render(area, &mut buffer);
    assert_eq!(buffer[(0, 0)].symbol(), "a");
    assert_eq!(buffer[(1, 0)].symbol(), "b");
    assert_eq!(buffer[(2, 0)].symbol(), "\u{1f469}\u{200d}\u{1f4bb}");
    let mut selection = TranscriptSelection::default();
    selection.update_snapshot(&rows, area, 0);
    assert!(selection.start(ScreenPosition::new(0, 0)));
    assert!(selection.drag(ScreenPosition::new(4, 0)));
    assert_eq!(selection.selected_text().as_deref(), Some(expected));
    selection.highlight_visible_range(&mut buffer);
    for x in 0..4 {
        assert!(
            buffer[(x, 0)]
                .modifier
                .contains(ratatui::style::Modifier::REVERSED)
        );
    }
}

#[test]
fn canonical_rows_preserve_cross_span_unicode_boundaries() {
    let rows = TranscriptRows::from_visual_lines(vec![Line::from(vec![
        Span::styled("a", Style::default().fg(ratatui::style::Color::Red)),
        Span::raw("\u{301}\u{1f469}"),
        Span::raw("\u{200d}\u{1f4bb}"),
    ])]);
    let row = rows.get(0).expect("row");
    let area = Rect::new(0, 0, 8, 1);
    let mut buffer = Buffer::empty(area);
    row.line.clone().render(area, &mut buffer);
    assert_eq!(buffer[(0, 0)].symbol(), "a\u{301}");
    assert_eq!(buffer[(1, 0)].symbol(), "\u{1f469}\u{200d}\u{1f4bb}");
    assert_eq!(buffer[(0, 0)].fg, ratatui::style::Color::Red);
    assert_eq!(row.width, 3);
}

#[test]
fn startup_truncation_obeys_unicode_boundaries() {
    for source in [
        "\u{754c}".repeat(12),
        "\u{1f469}\u{200d}\u{1f4bb}".repeat(8),
        "a\u{301}".repeat(20),
        "x\u{200b}y\u{1b}[31m\nlong label".to_string(),
    ] {
        for width in 0..24 {
            for clipped in [
                super::render::truncate_for_startup_card(&source, width),
                super::render::truncate_path_middle(&source, width),
            ] {
                assert!(
                    display_width(&clipped) <= width,
                    "width {width}: {clipped:?}"
                );
                assert!(!clipped.chars().any(char::is_control));
            }
        }
    }
}

#[test]
fn shared_editors_keep_unicode_boundaries_and_input_ownership() {
    for overlay in [
        None,
        Some(Overlay::CommandPalette),
        Some(Overlay::ModelSearch),
        Some(Overlay::BaseUrlEditor),
        Some(Overlay::ApiKeyEditor(ApiKeyTarget::OpenAiCompatible)),
        Some(Overlay::ModelNameEditor),
        Some(Overlay::OpenAiProfileLabelEditor),
    ] {
        let temp = tempdir().expect("tempdir");
        let mut app = app_in(&temp);
        app.overlay = overlay;
        app.insert_active_input_text("x\u{1f469}\u{200d}\u{1f4bb}");
        app.move_active_input_cursor_left();
        app.move_active_input_cursor_right();
        app.backspace_active_input();
        let (text, cursor) = match overlay {
            None | Some(Overlay::CommandPalette) => {
                (app.bottom_pane.input.as_str(), app.composer_cursor_offset())
            }
            Some(Overlay::ModelSearch) => (
                app.model_search_query.as_str(),
                app.model_search_cursor_offset(),
            ),
            Some(Overlay::BaseUrlEditor) => {
                (app.base_url_input.as_str(), app.base_url_cursor_offset())
            }
            Some(Overlay::ApiKeyEditor(_)) => {
                (app.api_key_input.as_str(), app.api_key_cursor_offset())
            }
            Some(Overlay::ModelNameEditor) => (
                app.model_name_input.as_str(),
                app.model_name_cursor_offset(),
            ),
            Some(Overlay::OpenAiProfileLabelEditor) => (
                app.openai_profile_label_input.as_str(),
                app.openai_profile_label_cursor_offset(),
            ),
            Some(other) => panic!("unexpected test overlay: {other:?}"),
        };
        assert_eq!((text, cursor), ("x", 1), "{overlay:?}");
        if overlay.is_some_and(|overlay| overlay != Overlay::CommandPalette) {
            assert!(app.bottom_pane.input.is_empty());
        }
    }
}

#[test]
fn edits_repair_stale_and_newly_joined_unicode_boundaries() {
    let temp = tempdir().expect("tempdir");
    let mut app = app_in(&temp);
    app.set_input("xa\u{301}y".to_string());
    app.bottom_pane.input_cursor_offset = Some(2);
    app.insert_active_input_text("b");
    assert_eq!(app.bottom_pane.input, "xba\u{301}y");
    assert_eq!(app.composer_cursor_offset(), 2);
    for backwards in [false, true] {
        app.set_input("\u{1f1f8}x\u{1f1ec}".to_string());
        if backwards {
            app.bottom_pane.input_cursor_offset = Some(2);
            app.backspace_active_input();
        } else {
            app.bottom_pane.input_cursor_offset = Some(1);
            app.delete_forward_active_input();
        }
        assert_eq!(app.bottom_pane.input, "\u{1f1f8}\u{1f1ec}");
        assert_eq!(app.bottom_pane.input_cursor_offset, Some(0));
    }
}

#[test]
fn display_projection_is_idempotent_at_unicode_boundaries() {
    let red = Style::default().fg(ratatui::style::Color::Red);
    let source = Line::from(vec![
        Span::styled("\u{1f44d}", red),
        Span::raw("\u{200b}\u{1f3fd}"),
    ]);
    let rows = TranscriptRows::from_visual_lines(vec![source]);
    let row = rows.get(0).expect("row");
    assert_eq!(row.text, "\u{1f44d}\u{1f3fd}");
    let normalized = super::display_sanitize::sanitize_display_line_segments(&row.line);
    assert_eq!(normalized, row.line);
    let area = Rect::new(0, 0, 8, 1);
    let mut buffer = Buffer::empty(area);
    row.line.clone().render(area, &mut buffer);
    assert_eq!(buffer[(0, 0)].symbol(), row.text);
    assert_eq!(row.width, 2);
}

#[test]
fn terminal_cell_width_matches_shared_unicode_boundaries() {
    for source in [
        "\u{754c}",
        "a\u{301}",
        "\u{1f469}\u{200d}\u{1f4bb}",
        "\u{ff9e}",
        "\u{ff76}\u{ff9f}",
        "a\u{200b}b",
        "a\tb",
    ] {
        let line = super::display_sanitize::sanitize_display_line_segments(&Line::from(source));
        let area = Rect::new(0, 0, 16, 1);
        let mut buffer = Buffer::empty(area);
        let (end, _) = buffer.set_line(0, 0, &line, area.width);
        assert_eq!(
            display_width(&line.to_string()),
            usize::from(end),
            "{source:?}"
        );
        for width in 1..8 {
            for row in super::transcript_text::wrap_line(&line, width) {
                assert!(display_width(&row.to_string()) <= usize::from(width));
            }
        }
    }
}

#[test]
fn goal_restore_rejects_overflow_at_unicode_boundary_checkpoint() {
    for field in ["token_budget", "tokens_used", "turns_completed"] {
        let temp = tempdir().expect("tempdir");
        let root = temp.path().join("repo");
        let state_root = root.join(".rara");
        std::fs::create_dir_all(&state_root).expect("state root");
        let state_db = Arc::new(
            rara_state::state_db::StateDb::new_for_root_dir(state_root.clone()).expect("state db"),
        );
        let agent = crate::agent::Agent::new(
            rara_tools::tool::ToolManager::new(),
            Arc::new(crate::llm::MockLlm),
            Arc::new(rara_memory::memory_handle::MemoryHandle::new(
                &state_root.join("memory").display().to_string(),
            )),
            Arc::new(crate::session::SessionManager {
                storage_dir: state_root.join("rollouts"),
                legacy_storage_dir: state_root.join("sessions"),
            }),
            Arc::new(crate::workspace::WorkspaceMemory::from_paths(
                root, state_root,
            )),
        );
        let mut app = app_in(&temp);
        app.attach_state_db(state_db.clone());
        app.apply_runtime_snapshot(
            &agent,
            crate::runtime_client::RuntimeClient::extension_snapshot_for_agent(&agent, 0),
        );
        let mut snapshot = json!({"objective": "stored objective", "status": "Paused", "token_budget": 100, "tokens_used": 3, "turns_completed": 2});
        snapshot[field] = json!(u64::from(u32::MAX) + 1);
        state_db
            .save_goal(&agent.session_id, &snapshot)
            .expect("save goal");
        let stored = state_db.load_goal(&agent.session_id).expect("stored goal");
        app.goal = Some(RalphGoal::new("previous thread".into(), None));
        *app.goal_handle.write().expect("goal lock") = app.goal.clone();
        let session_id = agent.session_id.clone();
        let mut agent_slot = Some(agent);
        super::session_restore::restore_thread_by_id(&session_id, &mut app, &mut agent_slot)
            .expect("restore thread");
        assert!(
            app.goal.is_none(),
            "{field} must not wrap or become unlimited"
        );
        assert!(app.goal_handle.read().expect("goal lock").is_none());
        assert!(
            app.bottom_pane
                .notice
                .as_deref()
                .expect("notice")
                .contains(field)
        );
        assert_eq!(state_db.load_goal(&session_id), Some(stored));
        snapshot[field] = json!(u32::MAX);
        state_db
            .save_goal(&session_id, &snapshot)
            .expect("save valid goal");
        super::session_restore::restore_thread_by_id(&session_id, &mut app, &mut agent_slot)
            .expect("restore exact limit");
        let goal = app.goal.as_ref().expect("valid restored goal");
        assert_eq!(
            goal.token_budget.map(u64::from),
            snapshot["token_budget"].as_u64()
        );
        assert_eq!(
            u64::from(goal.tokens_used),
            snapshot["tokens_used"].as_u64().expect("used")
        );
        assert_eq!(
            u64::from(goal.turns_completed),
            snapshot["turns_completed"].as_u64().expect("turns")
        );
        assert_eq!(goal.status, super::state::GoalStatus::Paused);
    }
}
