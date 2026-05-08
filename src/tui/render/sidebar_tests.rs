use ratatui::{layout::Rect, style::Color, text::Line};
use tempfile::tempdir;

use crate::config::ConfigManager;
use crate::tui::state::{
    PendingInteractionSnapshot, RuntimeSnapshot, TuiApp,
};

use super::{
    format_token_count, push_budget_bar, push_child_sessions, push_context_summary,
    push_model_badge, push_session_info,
};

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

#[test]
fn push_session_info_shows_id_cwd_branch() {
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
    assert!(text.contains("abcdefgh…5678"), "should contain shortened session id");
    assert!(text.contains("/home/user/project"), "should contain cwd");
    assert!(text.contains("main"), "should contain branch");
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

    let text: String = lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
    assert!(text.contains("RARA"), "should show RARA when no session id");
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

    let text: String = lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
    assert!(text.contains("abc"), "short id should appear as-is");
}

// ── push_model_badge ────────────────────────────────────────────────

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

    let text: String = lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
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

    let text: String = lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
    assert!(text.contains("anthropic"), "should show provider");
    assert!(text.contains("default"), "should fall back to default model");
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

    let text: String = lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
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

    let text: String = lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
    assert!(!text.contains("reasoning"), "should not mention reasoning when unset");
}

// ── push_budget_bar ─────────────────────────────────────────────────

#[test]
fn push_budget_bar_skips_when_no_context_window() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.snapshot = RuntimeSnapshot {
        context_window_tokens: None,
        ..RuntimeSnapshot::default()
    };

    let mut lines = Vec::new();
    push_budget_bar(&mut lines, &app, 30);
    assert!(lines.is_empty(), "should be empty when no context window");
}

#[test]
fn push_budget_bar_renders_segments_and_stats() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.snapshot = RuntimeSnapshot {
        context_window_tokens: Some(32768),
        stable_instructions_budget: 2000,
        workspace_prompt_budget: 500,
        active_turn_budget: 10000,
        compacted_history_budget: 4000,
        retrieved_memory_budget: 1500,
        remaining_input_budget: Some(14768),
        total_input_tokens: 222,
        total_output_tokens: 44,
        total_cache_hit_tokens: 100,
        total_cache_miss_tokens: 10,
        ..RuntimeSnapshot::default()
    };

    let mut lines = Vec::new();
    push_budget_bar(&mut lines, &app, 38);

    let text: String = lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
    // Budget bar uses unicode block chars — just check labels appear.
    assert!(text.contains("sys"), "should show sys label");
    assert!(text.contains("ws"), "should show ws label");
    assert!(text.contains("act"), "should show act label");
    assert!(text.contains("hist"), "should show hist label");
    assert!(text.contains("mem"), "should show mem label");
    assert!(text.contains("free"), "should show free label");

    // Token stats
    assert!(text.contains("32.8k"), "should show context window size");
    assert!(text.contains("222"), "should show input tokens");
    assert!(text.contains("44"), "should show output tokens");
    assert!(text.contains("100"), "should show cache hit tokens");
    assert!(text.contains("10"), "should show cache miss tokens");
}

#[test]
fn push_budget_bar_no_remaining_input_budget() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.snapshot = RuntimeSnapshot {
        context_window_tokens: Some(16384),
        remaining_input_budget: None,
        ..RuntimeSnapshot::default()
    };

    let mut lines = Vec::new();
    push_budget_bar(&mut lines, &app, 38);

    // Should still render without free label (no remaining_input_budget).
    let text: String = lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
    assert!(!text.is_empty(), "should render budget bar");
}

// ── push_child_sessions ─────────────────────────────────────────────

#[test]
fn push_child_sessions_empty_when_no_children() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");

    let mut lines = Vec::new();
    push_child_sessions(&mut lines, &app);
    assert!(lines.is_empty(), "no output when zero children");
}

#[test]
fn push_child_sessions_shows_count() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.snapshot.pending_interactions = vec![
        PendingInteractionSnapshot {
            kind: crate::tui::state::InteractionKind::RequestInput,
            title: "sub-1".into(),
            summary: "".into(),
            options: vec![("ok".into(), "OK".into())],
            note: None,
            approval: None,
            source: None,
        },
        PendingInteractionSnapshot {
            kind: crate::tui::state::InteractionKind::RequestInput,
            title: "sub-2".into(),
            summary: "".into(),
            options: vec![("ok".into(), "OK".into())],
            note: None,
            approval: None,
            source: None,
        },
    ];

    let mut lines = Vec::new();
    push_child_sessions(&mut lines, &app);

    let text: String = lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
    assert!(text.contains("2 active"), "should show sub-agent count");
}

// ── push_context_summary ────────────────────────────────────────────

#[test]
fn push_context_summary_shows_turns() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.snapshot.history_len = 12;

    let mut lines = Vec::new();
    push_context_summary(&mut lines, &app);

    let text: String = lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
    assert!(text.contains("12 turns"), "should show turn count");
    assert!(!text.contains("compacted"), "should not show compaction when zero");
}

#[test]
fn push_context_summary_shows_compaction() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.snapshot.history_len = 8;
    app.snapshot.compaction_count = 3;

    let mut lines = Vec::new();
    push_context_summary(&mut lines, &app);

    let text: String = lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
    assert!(text.contains("8 turns"), "should show turn count");
    assert!(text.contains("compacted"), "should show compaction");
    assert!(text.contains("3 times"), "should show compaction count");
}
