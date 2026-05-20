use ratatui::{layout::Rect, style::Color, text::Line};
use tempfile::tempdir;

use super::{
    push_child_sessions, push_context_summary, push_lsp_status, push_model_badge,
    push_session_info, push_todo_section,
};
use crate::config::ConfigManager;
use crate::context::TodoContextView;
use crate::todo::TodoSummary;
use crate::tui::state::{
    InteractionKind, PendingInteractionSnapshot, RuntimeSnapshot, TranscriptEntry, TranscriptTurn,
    TuiApp,
};
use crate::tui::status_display::format_token_count;

// ── format_token_count ──────────────────────────────────────────────

#[test]
fn format_token_count_small() {
    assert_eq!(format_token_count(0), "0");
    assert_eq!(format_token_count(42), "42");
    assert_eq!(format_token_count(999), "999");
}

#[test]
fn format_token_count_kilo() {
    assert_eq!(format_token_count(1_000), "1.0k");
    assert_eq!(format_token_count(1_500), "1.5k");
    assert_eq!(format_token_count(999_999), "1000.0k"); // rounds to 1000.0k
}

#[test]
fn format_token_count_mega() {
    assert_eq!(format_token_count(1_000_000), "1.0M");
    assert_eq!(format_token_count(2_500_000), "2.5M");
}

// ── push_session_info ───────────────────────────────────────────────
// Now merges cwd + branch into a single compact line.

#[test]
fn push_session_info_shows_id_and_location() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.snapshot = RuntimeSnapshot {
        session_id: "abcdefgh12345678".into(),
        cwd: "/home/user/project".into(),
        branch: "main".into(),
        ..RuntimeSnapshot::default()
    };

    let mut lines = Vec::new();
    push_session_info(&mut lines, &app);

    let text: String = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("abcdefgh…5678"),
        "should contain shortened session id"
    );
    assert!(
        text.contains("/home/user/project :: main"),
        "should show cwd :: branch on one line"
    );
}

#[test]
fn push_session_info_empty_session_shows_rara() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.snapshot = RuntimeSnapshot {
        session_id: String::new(),
        ..RuntimeSnapshot::default()
    };

    let mut lines = Vec::new();
    push_session_info(&mut lines, &app);

    let text: String = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("RARA"), "should show RARA when no session id");
}

#[test]
fn push_session_info_no_branch_shows_cwd_only() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.snapshot = RuntimeSnapshot {
        cwd: "/home/user/project".into(),
        branch: String::new(),
        ..RuntimeSnapshot::default()
    };

    let mut lines = Vec::new();
    push_session_info(&mut lines, &app);
    let text: String = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("/home/user/project"), "should show cwd");
    assert!(
        !text.contains("::"),
        "no branch separator when branch empty"
    );
}

#[test]
fn push_session_info_short_session_id_not_truncated() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.snapshot = RuntimeSnapshot {
        session_id: "abc".into(),
        ..RuntimeSnapshot::default()
    };

    let mut lines = Vec::new();
    push_session_info(&mut lines, &app);

    let text: String = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("abc"), "short id should appear as-is");
}

// ── push_model_badge ────────────────────────────────────────────────
// Now includes "# Model" section header.

#[test]
fn push_model_badge_shows_provider_and_model() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.config.provider = "openai".into();
    app.config.model = Some("gpt-4o".into());

    let mut lines = Vec::new();
    push_model_badge(&mut lines, &app);

    let text: String = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("openai"), "should show provider");
    assert!(text.contains("gpt-4o"), "should show model");
}

#[test]
fn push_model_badge_falls_back_to_default_model() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.config.provider = "anthropic".into();
    app.config.model = None;

    let mut lines = Vec::new();
    push_model_badge(&mut lines, &app);

    let text: String = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("anthropic"), "should show provider");
    assert!(
        text.contains("default"),
        "should fall back to default model"
    );
}

#[test]
fn push_model_badge_shows_reasoning_effort_when_set() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.config.provider = "openai".into();
    app.config.reasoning_effort = Some("high".into());

    let mut lines = Vec::new();
    push_model_badge(&mut lines, &app);

    let text: String = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("high"), "should show reasoning effort");
}

#[test]
fn push_model_badge_no_reasoning_effort_when_none() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.config.provider = "openai".into();
    app.config.reasoning_effort = None;

    let mut lines = Vec::new();
    push_model_badge(&mut lines, &app);

    let text: String = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !text.contains("reasoning"),
        "should not mention reasoning when unset"
    );
}

// ── push_context_summary ────────────────────────────────────────────
// Replaces push_budget_bar. Shows tokens, turns, compaction in one line.

#[test]
fn push_context_summary_shows_tokens_turns_compaction() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.snapshot = RuntimeSnapshot {
        context_window_tokens: Some(16000),
        stable_instructions_budget: 9400,
        history_len: 42,
        compaction_count: 3,
        ..RuntimeSnapshot::default()
    };

    let mut lines = Vec::new();
    push_context_summary(&mut lines, &app);

    let text: String = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("Context"),
        "should show # Context section label"
    );
    assert!(
        text.contains("9.4k") && text.contains("16.0k") && text.contains("42 turns"),
        "should show token usage: 9.4k/16.0k"
    );
    assert!(text.contains("42 turns"), "should show turn count");
    assert!(
        text.contains("compacted 3×"),
        "should show compaction count"
    );
}

#[test]
fn push_context_summary_no_compaction() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.snapshot = RuntimeSnapshot {
        context_window_tokens: Some(16000),
        stable_instructions_budget: 1500,
        history_len: 1,
        compaction_count: 0,
        ..RuntimeSnapshot::default()
    };

    let mut lines = Vec::new();
    push_context_summary(&mut lines, &app);
    let text: String = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!text.contains("compacted"), "no compaction mention when 0");
}

#[test]
fn push_context_summary_no_context_window() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.snapshot = RuntimeSnapshot {
        context_window_tokens: None,
        stable_instructions_budget: 600,
        history_len: 5,
        ..RuntimeSnapshot::default()
    };

    let mut lines = Vec::new();
    push_context_summary(&mut lines, &app);
    assert!(
        lines
            .iter()
            .any(|l| l.to_string().contains("600 · 5 turns")),
        "shows current-turn history tokens without context window"
    );
}

#[test]
fn push_lsp_status_reports_not_initialized() {
    let temp = tempdir().unwrap();
    let app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");

    let mut lines = Vec::new();
    push_lsp_status(&mut lines, &app);

    let text: String = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("LSP"));
    assert!(text.contains("not initialized"));
}

#[test]
fn push_lsp_status_reports_no_detected_project_server() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.lsp_manager = Some(std::sync::Arc::new(crate::lsp_manager::LspManager::new(
        temp.path().to_path_buf(),
    )));

    let mut lines = Vec::new();
    push_lsp_status(&mut lines, &app);

    let text: String = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("LSP"));
    assert!(text.contains("no project server detected"));
}

#[test]
fn push_todo_section_shows_progress_and_items() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.snapshot = RuntimeSnapshot {
        todo: TodoContextView {
            summary: TodoSummary {
                total: 4,
                pending: 1,
                in_progress: 1,
                completed: 1,
                cancelled: 1,
                active_item: Some("Run focused regression test".into()),
            },
            updated_at: Some(1_777_584_000),
            items: vec![
                (
                    "todo-1".into(),
                    "completed".into(),
                    "Reproduce failing behavior".into(),
                ),
                (
                    "todo-2".into(),
                    "in_progress".into(),
                    "Run focused regression test".into(),
                ),
                (
                    "todo-3".into(),
                    "pending".into(),
                    "Check nearby side effects".into(),
                ),
                (
                    "todo-4".into(),
                    "cancelled".into(),
                    "Broader cleanup".into(),
                ),
            ],
        },
        ..RuntimeSnapshot::default()
    };

    let mut lines = Vec::new();
    assert!(push_todo_section(&mut lines, &app));

    let text: String = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Todo"));
    assert!(text.contains("1 / 4 · 2 open"));
    assert!(text.contains("● Run focused regression test"));
    assert!(text.contains("☑ Reproduce failing behavior"));
    assert!(text.contains("● Run focused regression test"));
    assert!(text.contains("☐ Check nearby side effects"));
    assert!(text.contains("✗ Broader cleanup"));
}

// ── push_child_sessions ─────────────────────────────────────────────
// Now shows "# Sub-agents" section header + each title (running).

#[test]
fn push_child_sessions_shows_titles_running() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.snapshot = RuntimeSnapshot {
        pending_interactions: vec![
            PendingInteractionSnapshot {
                kind: InteractionKind::Approval,
                title: "explore-sidebar".into(),
                ..Default::default()
            },
            PendingInteractionSnapshot {
                kind: InteractionKind::PlanApproval,
                title: "plan-refactor".into(),
                ..Default::default()
            },
        ],
        ..RuntimeSnapshot::default()
    };

    let mut lines = Vec::new();
    push_child_sessions(&mut lines, &app);
    let text: String = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("Sub-agents"),
        "should show # Sub-agents section label"
    );
    assert!(
        text.contains("explore-sidebar (running)"),
        "should show sub-agent title + (running)"
    );
}

#[test]
fn push_child_sessions_skip_when_empty() {
    let temp = tempdir().unwrap();
    let app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let mut lines = Vec::new();
    push_child_sessions(&mut lines, &app);
    assert!(lines.is_empty(), "empty when no pending interactions");
}

// ── PendingInteractionSnapshot default helper ───────────────────────

impl Default for PendingInteractionSnapshot {
    fn default() -> Self {
        Self {
            kind: InteractionKind::RequestInput,
            title: String::new(),
            summary: String::new(),
            options: Vec::new(),
            note: None,
            approval: None,
            source: None,
        }
    }
}

// ─── Section header check ───────────────────────────────────────────

#[test]
fn section_header_appears_in_sidebar() {
    // Verify the sidebar calls section_label for each area.
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.snapshot = RuntimeSnapshot {
        session_id: "sess1".into(),
        cwd: "/tmp".into(),
        ..RuntimeSnapshot::default()
    };
    app.config.provider = "test".into();
    app.config.model = Some("m".into());

    // Collect all sidebar lines by calling each push fn.
    let mut lines: Vec<Line<'static>> = Vec::new();
    push_session_info(&mut lines, &app);
    lines.push(Line::from(""));
    push_model_badge(&mut lines, &app);
    lines.push(Line::from(""));
    push_context_summary(&mut lines, &app);
    lines.push(Line::from(""));
    push_child_sessions(&mut lines, &app);

    let text: String = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("Context"), "sidebar has Context section");
}
