use ratatui::{layout::Rect, style::Color, text::Line};
use tempfile::tempdir;

use super::{
    format_token_count, push_child_sessions, push_context_summary, push_files_in_context,
    push_model_badge, push_session_info,
};
use crate::config::ConfigManager;
use crate::tui::state::{
    InteractionKind, PendingInteractionSnapshot, RuntimeSnapshot, TranscriptEntry, TranscriptTurn,
    TuiApp,
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
    assert!(!text.contains("::"), "no branch separator when branch empty");
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
    assert!(text.contains("Model"), "should show # Model section label");
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
        total_input_tokens: 8200,
        total_output_tokens: 1200,
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
    assert!(text.contains("Context"), "should show # Context section label");
    assert!(
        text.contains("9.4k/16.0k tokens"),
        "should show token usage: 9.4k/16.0k"
    );
    assert!(text.contains("42 turns"), "should show turn count");
    assert!(text.contains("compacted 3×"), "should show compaction count");
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
        total_input_tokens: 1000,
        total_output_tokens: 500,
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
        total_input_tokens: 500,
        total_output_tokens: 100,
        history_len: 5,
        ..RuntimeSnapshot::default()
    };

    let mut lines = Vec::new();
    push_context_summary(&mut lines, &app);
    assert!(
        lines
            .iter()
            .any(|l| l.to_string().contains("600 tokens")),
        "shows total tokens without context window"
    );
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

// ── push_files_in_context ───────────────────────────────────────────
// Extracts read_file / apply_patch / write_file paths from transcript.

#[test]
fn push_files_in_context_empty_when_no_file_tools() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.committed_turns = vec![TranscriptTurn {
        entries: vec![TranscriptEntry::new(
            "Assistant",
            "Let me think about this.",
        )],
    }];

    let mut lines = Vec::new();
    push_files_in_context(&mut lines, &app);
    assert!(lines.is_empty(), "empty when no file-tool entries");
}

#[test]
fn push_files_in_context_shows_read_and_changed() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.committed_turns = vec![TranscriptTurn {
        entries: vec![
            TranscriptEntry::new("Tool", "read_file src/main.rs"),
            TranscriptEntry::new("Tool", "read_file crates/lib.rs"),
            TranscriptEntry::new("Tool", "apply_patch src/main.rs"),
            TranscriptEntry::new("Tool", "write_file docs/new.md"),
        ],
    }];

    let mut lines = Vec::new();
    push_files_in_context(&mut lines, &app);
    let text: String = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Files"), "should show # Files section label");
    assert!(text.contains("Read:"), "should have Read subsection");
    assert!(text.contains("src/main.rs"), "should show read file");
    assert!(text.contains("crates/lib.rs"), "should show read file");
    assert!(text.contains("Changed:"), "should have Changed subsection");
    assert!(text.contains("docs/new.md"), "should show changed file");
}

#[test]
fn push_files_in_context_deduplicates() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.committed_turns = vec![TranscriptTurn {
        entries: vec![
            TranscriptEntry::new("Tool", "read_file src/main.rs"),
            TranscriptEntry::new("Tool", "read_file src/main.rs"),
        ],
    }];

    let mut lines = Vec::new();
    push_files_in_context(&mut lines, &app);
    let text: String = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    // Only one occurrence of src/main.rs
    let count = text.matches("src/main.rs").count();
    assert_eq!(count, 1, "duplicate file paths are collapsed");
}

#[test]
fn push_files_in_context_truncates_at_five() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    let files: Vec<_> = (1..=8)
        .map(|i| TranscriptEntry::new("Tool", format!("read_file src/file{i}.rs")))
        .collect();
    app.committed_turns = vec![TranscriptTurn { entries: files }];

    let mut lines = Vec::new();
    push_files_in_context(&mut lines, &app);
    let text: String = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("... and 3 more"),
        "should truncate at 5 with overflow count"
    );
}

#[test]
fn push_files_in_context_read_only_no_changed() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.committed_turns = vec![TranscriptTurn {
        entries: vec![TranscriptEntry::new("Tool", "read_file src/main.rs")],
    }];

    let mut lines = Vec::new();
    push_files_in_context(&mut lines, &app);
    let text: String = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Read:"));
    assert!(!text.contains("Changed:"));
}

#[test]
fn push_files_in_context_includes_active_turn() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.active_turn = TranscriptTurn {
        entries: vec![TranscriptEntry::new("Tool", "read_file crates/tool.rs")],
    };

    let mut lines = Vec::new();
    push_files_in_context(&mut lines, &app);
    let text: String = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("crates/tool.rs"),
        "should include files from active turn"
    );
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
    lines.push(Line::from(""));
    push_files_in_context(&mut lines, &app);

    let text: String = lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    // Model, Context, and Files sections should exist.
    assert!(text.contains("Model"), "sidebar has Model section");
    assert!(text.contains("Context"), "sidebar has Context section");
    // Files only appears when there are file-tool entries; in this test none.
}
