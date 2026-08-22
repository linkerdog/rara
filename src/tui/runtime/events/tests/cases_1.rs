use rara_tools::tool::ToolOutputStream;
use serde_json::json;
use tempfile::tempdir;

use super::helpers::{
    format_apply_patch_result, format_apply_patch_use, format_tool_progress, format_tool_result,
    format_tool_use, is_oauth_prompt_message, planning_note_lines, scrub_internal_control_tokens,
    subagent_request_input,
};
use super::{apply_tui_event, format_memory_event_notice, runtime_event_from_agent_event};
use crate::agent::{AgentEvent, AgentExecutionMode};
use crate::config::ConfigManager;
use crate::control_tokens::has_pending_internal_control_context;
use crate::runtime_control::{MemoryEvent, MemoryRecordSummary, RuntimeEvent, RuntimeProvenance};
use crate::session_promotion::{
    SessionShardPromotionDecision, SessionShardPromotionOutcome, SessionShardPromotionPlan,
    SessionShardPromotionSkipReason, SessionShardPromotionTrigger,
};
use crate::tui::state::{ActivePendingInteractionKind, TranscriptEntryPayload};
use crate::tui::state::{RuntimePhase, TuiApp, TuiEvent};
use crate::tui::terminal_event::{TerminalEvent, TerminalTarget};

#[test]
fn runtime_agent_event_preserves_structured_semantics_for_tui() {
    let event = runtime_event_from_agent_event(
        AgentEvent::ToolUse {
            call_id: "call-1".into(),
            name: "write_file".into(),
            input: json!({"path": "src/runtime.rs", "content": "fn main() {}"}),
        },
        RuntimeProvenance::local_tui("session-1"),
    );

    assert!(matches!(
        event,
        TuiEvent::Runtime(envelope)
            if matches!(envelope.event, RuntimeEvent::Tool(_))
    ));
}

#[test]
fn structured_tool_event_updates_running_action_without_role_parsing() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
    let event = TuiEvent::Runtime(Box::new(crate::runtime_control::RuntimeControlEvent {
        event_id: "event-1".into(),
        provenance: RuntimeProvenance::local_tui("session-1"),
        turn_id: None,
        sequence: 1,
        event: RuntimeEvent::Tool(crate::runtime_control::ToolEvent::Use {
            call_id: Some("call-1".into()),
            name: "write_file".into(),
            input: json!({"path": "src/runtime.rs", "content": "fn main() {}"}),
        }),
    }));

    apply_tui_event(&mut app, event);

    assert_eq!(
        app.active_live.running_actions,
        vec!["Write src/runtime.rs".to_string()]
    );
    assert_eq!(app.active_turn.entries[0].role, "Tool");
    match app.active_turn.entries[0].payload.as_ref() {
        Some(crate::tui::state::TranscriptEntryPayload::Tool(payload)) => {
            assert_eq!(payload.call_id.as_deref(), Some("call-1"));
            assert_eq!(payload.name, "write_file");
            assert_eq!(
                payload.status,
                crate::tui::state::ToolTranscriptStatus::Running
            );
        }
        payload => panic!("expected structured tool payload, got {payload:?}"),
    }
}

#[test]
fn structured_compaction_event_becomes_a_typed_transcript_entry() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
    apply_tui_event(
        &mut app,
        TuiEvent::Runtime(Box::new(crate::runtime_control::RuntimeControlEvent {
            event_id: "compact-1".into(),
            provenance: RuntimeProvenance::local_tui("session-1"),
            turn_id: None,
            sequence: 1,
            event: RuntimeEvent::Session(crate::runtime_control::SessionEvent::Compacted {
                count: 2,
                before_tokens: 12_000,
                after_tokens: 4_500,
                summary: "Retained the active task and recent files.".into(),
                recent_files: vec!["src/runtime_control.rs".into()],
            }),
        })),
    );

    assert_eq!(app.snapshot.compaction_count, 2);
    assert_eq!(app.active_turn.entries[0].role, "Compaction");
    match app.active_turn.entries[0].payload.as_ref() {
        Some(TranscriptEntryPayload::Compaction(payload)) => {
            assert_eq!(payload.count, 2);
            assert_eq!(payload.before_tokens, 12_000);
            assert_eq!(payload.after_tokens, 4_500);
            assert_eq!(payload.recent_files, vec!["src/runtime_control.rs"]);
        }
        payload => panic!("expected compaction payload, got {payload:?}"),
    }
}

#[test]
fn parses_delegated_request_input_from_subagent_result() {
    let parsed = subagent_request_input(
        "plan_agent refine the workspace logic\nrequest_user_input: Which discovery strategy should we keep?\noption: Minimal | Keep the current root-level files.\noption: Generic | Scan all instruction markdown files.\nnote: We need one product decision before editing.",
    )
    .expect("delegated request input should parse");

    assert_eq!(parsed.question, "Which discovery strategy should we keep?");
    assert_eq!(parsed.options.len(), 2);
    assert_eq!(parsed.options[0].0, "Minimal");
    assert_eq!(parsed.options[1].0, "Generic");
    assert_eq!(
        parsed.note.as_deref(),
        Some("We need one product decision before editing.")
    );
}

#[test]
fn memory_action_event_becomes_renderable_system_notice() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
    apply_tui_event(
        &mut app,
        runtime_event_from_agent_event(
            AgentEvent::MemoryAction {
                message: "Memory · querying workspace memory".into(),
            },
            RuntimeProvenance::local_tui("session-1"),
        ),
    );
    assert_eq!(
        app.active_turn
            .entries
            .last()
            .expect("memory entry")
            .message,
        "Memory · querying workspace memory"
    );
}

#[test]
fn memory_query_notice_reports_count_without_record_content() {
    let notice = format_memory_event_notice(&MemoryEvent::RecordsQueried {
        query: "repo notes".into(),
        records: vec![MemoryRecordSummary {
            id: "memory-1234567890".into(),
            title: "Useful title".into(),
            content: "secret memory content".into(),
            labels: vec!["project".into()],
            importance_basis_points: 8000,
            pinned: false,
            scope: "workspace".into(),
            session_id: None,
            thread_id: None,
        }],
    });

    assert_eq!(
        notice,
        "Memory · queried records for \"repo notes\": 1 result"
    );
    assert!(!notice.contains("secret memory content"));
    assert!(!notice.contains("Useful title"));
}

#[test]
fn memory_query_notice_sanitizes_query_preview() {
    let notice = format_memory_event_notice(&MemoryEvent::RecordsQueried {
        query: format!("{}\n{}", "token=sk-test-secret", "word ".repeat(40)),
        records: Vec::new(),
    });

    assert!(notice.starts_with("Memory · queried records for \""));
    assert!(notice.ends_with("\": 0 results"));
    assert!(!notice.contains("sk-test-secret"));
    assert!(!notice.contains('\n'));
    assert!(notice.len() < 180);
}

#[test]
fn memory_promotion_notice_uses_readable_outcome() {
    let notice = format_memory_event_notice(&MemoryEvent::SessionShardPromotionObserved {
        outcome: SessionShardPromotionOutcome {
            plan: SessionShardPromotionPlan {
                session_id: "session-a".into(),
                trigger: SessionShardPromotionTrigger::RuntimeControl,
                checkpoint_count: 3,
                min_checkpoints: 2,
                max_checkpoints: 8,
                decision: SessionShardPromotionDecision::Skipped {
                    reason: SessionShardPromotionSkipReason::BelowMinCheckpoints,
                },
            },
            promoted_count: 0,
        },
    });

    assert_eq!(
        notice,
        "Memory · skipped session shard promotion: below minimum checkpoints with 3 checkpoints"
    );
    assert!(!notice.contains("SessionShardPromotionOutcome"));
}

#[test]
fn parses_delegated_request_input_from_spawn_agent_result() {
    let parsed = subagent_request_input(
        "spawn_agent worker: Need a decision\nrequest_user_input: Which branch should continue?\noption: Main | Continue on main.",
    )
    .expect("spawn_agent request input should parse");

    assert_eq!(parsed.question, "Which branch should continue?");
    assert_eq!(
        parsed.options,
        vec![("Main".into(), "Continue on main.".into())]
    );
}

#[test]
fn explore_agent_result_with_request_input_records_note_and_pending_question() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");

    apply_tui_event(
        &mut app,
        TuiEvent::Transcript {
            role: "Tool Result",
            message: "explore_agent Found two workspace discovery paths.\nrequest_user_input: Which discovery strategy should we keep?\noption: Minimal | Keep root-level files only.\noption: Generic | Scan instruction markdown files.".into(),
        },
    );

    assert_eq!(
        app.active_live.exploration_notes,
        vec!["Sub-agent summary: Found two workspace discovery paths.".to_string()]
    );
    let pending = app
        .pending_request_input()
        .expect("delegated request should become pending input");
    assert_eq!(pending.source.as_deref(), Some("explore_agent"));
    assert_eq!(pending.title, "Which discovery strategy should we keep?");
    assert_eq!(pending.options.len(), 2);
    assert_eq!(
        app.active_pending_interaction().map(|item| item.kind),
        Some(ActivePendingInteractionKind::ExplorationQuestion)
    );
}

#[test]
fn plan_agent_result_with_request_input_records_note_and_pending_question() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");

    apply_tui_event(
        &mut app,
        TuiEvent::Transcript {
            role: "Tool Result",
            message: "plan_agent Need to choose a rollout boundary.\nrequest_user_input: Which phase should land first?\noption: Runtime | Wire the runtime path first.\noption: UI | Start with visibility.".into(),
        },
    );

    assert_eq!(
        app.active_live.planning_notes,
        vec!["Sub-agent summary: Need to choose a rollout boundary.".to_string()]
    );
    let pending = app
        .pending_request_input()
        .expect("delegated request should become pending input");
    assert_eq!(pending.source.as_deref(), Some("plan_agent"));
    assert_eq!(pending.title, "Which phase should land first?");
    assert_eq!(pending.options.len(), 2);
    assert_eq!(
        app.active_pending_interaction().map(|item| item.kind),
        Some(ActivePendingInteractionKind::PlanningQuestion)
    );
}

#[test]
fn spawn_agent_result_with_request_input_records_subagent_question() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");

    apply_tui_event(
        &mut app,
        TuiEvent::Transcript {
            role: "Tool Result",
            message: "spawn_agent Need user input before continuing.\nrequest_user_input: Which branch should continue?\noption: Current | Continue on the current branch.\noption: New | Create a new branch.".into(),
        },
    );

    assert!(app.active_live.exploration_notes.is_empty());
    assert!(app.active_live.planning_notes.is_empty());
    let pending = app
        .pending_request_input()
        .expect("delegated request should become pending input");
    assert_eq!(pending.source.as_deref(), Some("spawn_agent"));
    assert_eq!(pending.title, "Which branch should continue?");
    assert_eq!(pending.options.len(), 2);
    assert_eq!(
        app.active_pending_interaction().map(|item| item.kind),
        Some(ActivePendingInteractionKind::SubAgentQuestion)
    );
}

#[test]
fn planning_note_lines_drop_meta_and_mutating_chatter() {
    let notes = planning_note_lines(
        "I will use apply_patch on crates/instructions/src/workspace.rs.\nThe current discovery is hardcoded to root-level markdown files.\nThis is the final step: applying the patch.",
    );
    assert_eq!(
        notes,
        vec!["The current discovery is hardcoded to root-level markdown files.".to_string()]
    );
}

#[test]
fn scrub_internal_channel_markers_preserves_text_boundaries() {
    let cleaned = scrub_internal_control_tokens(
        "Inspecting prompt sources.<channel|>I have a concrete implementation plan.",
    );
    assert_eq!(
        cleaned,
        "Inspecting prompt sources.\nI have a concrete implementation plan."
    );
}

#[test]
fn scrub_internal_control_tokens_removes_agent_runtime_blocks() {
    let cleaned = scrub_internal_control_tokens(
        "Before\n<agent_runtime>\n{\"phase\":\"tool_results_available\"}\n</agent_runtime>\nAfter",
    );

    assert_eq!(cleaned.trim(), "Before\n\nAfter");
    assert!(!cleaned.contains("agent_runtime"));
    assert!(!cleaned.contains("tool_results_available"));
}

#[test]
fn scrub_internal_control_tokens_preserves_inline_runtime_block_boundaries() {
    let cleaned = scrub_internal_control_tokens(
        "Before<agent_runtime>{\"phase\":\"tool_results_available\"}</agent_runtime>After",
    );

    assert_eq!(cleaned, "Before\nAfter");
    assert!(!cleaned.contains("agent_runtime"));
    assert!(!cleaned.contains("tool_results_available"));
}

#[test]
fn scrub_internal_control_tokens_removes_open_runtime_blocks() {
    let cleaned = scrub_internal_control_tokens(
        "Before\n<agent_runtime>\n{\"phase\":\"tool_results_available\"}",
    );

    assert_eq!(cleaned.trim(), "Before");
}

#[test]
fn scrub_internal_control_tokens_removes_folded_history_context_blocks() {
    let cleaned = scrub_internal_control_tokens(
        "Visible\n<rara_internal_history_context>\nassistant: historical tool request: name=read_file\n</rara_internal_history_context>\nDone",
    );

    assert_eq!(cleaned.trim(), "Visible\n\nDone");
    assert!(!cleaned.contains("historical tool request"));
}

#[test]
fn scrub_internal_control_tokens_removes_dsml_tool_blocks() {
    let cleaned = scrub_internal_control_tokens(
        "Before\n<｜DSML｜tool_calls>\n<｜DSML｜invoke name=\"apply_patch\">\n<｜DSML｜parameter name=\"path\" string=\"true\">src/lib.rs</｜DSML｜parameter>\n</｜DSML｜invoke>\n</｜DSML｜tool_calls>\nAfter",
    );

    assert_eq!(cleaned.trim(), "Before\n\nAfter");
    assert!(!cleaned.contains("DSML"));
    assert!(!cleaned.contains("apply_patch"));
}

#[test]
fn scrub_internal_control_tokens_removes_ascii_pipe_dsml_tool_blocks() {
    let cleaned = scrub_internal_control_tokens(
        "Before\n<|DSML|tool_calls>\n<|DSML|invoke name=\"read_file\">\n<|DSML|parameter name=\"path\" string=\"true\">src/lib.rs</|DSML|parameter>\n</|DSML|invoke>\n</|DSML|tool_calls>\nAfter",
    );

    assert_eq!(cleaned.trim(), "Before\n\nAfter");
    assert!(!cleaned.contains("DSML"));
    assert!(!cleaned.contains("read_file"));
}

#[test]
fn scrub_internal_control_tokens_removes_structured_dsml_tool_block_with_json_parameter() {
    let cleaned = scrub_internal_control_tokens(
        "Before\n<｜DSML｜tool_calls>\n<｜DSML｜invoke name=\"read_file\">\n<｜DSML｜parameter name=\"options\" string=\"false\">{\"path\":\"src/lib.rs\",\"limit\":20}</｜DSML｜parameter>\n</｜DSML｜invoke>\n</｜DSML｜tool_calls>\nAfter",
    );

    assert_eq!(cleaned.trim(), "Before\n\nAfter");
    assert!(!cleaned.contains("read_file"));
}

#[test]
fn scrub_internal_control_tokens_removes_dsml_after_thinking_like_deepseek_completion() {
    let cleaned = scrub_internal_control_tokens(
        "<think>The user wants weather. I should use the get_weather tool.</think>\n\n<｜DSML｜tool_calls>\n<｜DSML｜invoke name=\"get_weather\">\n<｜DSML｜parameter name=\"location\" string=\"true\">Beijing</｜DSML｜parameter>\n<｜DSML｜parameter name=\"unit\" string=\"true\">celsius</｜DSML｜parameter>\n</｜DSML｜invoke>\n</｜DSML｜tool_calls><｜end▁of▁sentence｜>",
    );

    assert_eq!(cleaned.trim(), "");
    assert!(!cleaned.contains("<think>"));
    assert!(!cleaned.contains("</think>"));
    assert!(!cleaned.contains("<｜DSML｜tool_calls>"));
    assert!(!cleaned.contains("<｜DSML｜invoke"));
    assert!(!cleaned.contains("<｜DSML｜parameter"));
    assert!(!cleaned.contains("<｜end▁of▁sentence｜>"));
}

#[test]
fn scrub_internal_control_tokens_preserves_literal_open_leading_think_block() {
    let cleaned = scrub_internal_control_tokens("<think>literal XML example still streaming");

    assert_eq!(cleaned, "<think>literal XML example still streaming");
}

#[test]
fn scrub_internal_control_tokens_removes_deepseek_open_leading_think_block() {
    let cleaned = scrub_internal_control_tokens(
        "<think>private reasoning still streaming<｜end▁of▁sentence｜>",
    );

    assert!(cleaned.is_empty());
}

#[test]
fn scrub_internal_control_tokens_removes_deepseek_closed_leading_think_block() {
    let cleaned = scrub_internal_control_tokens(
        "<think>private reasoning</think>\nVisible answer.<｜end▁of▁sentence｜>",
    );

    assert_eq!(cleaned.trim(), "Visible answer.");
    assert!(!cleaned.contains("private reasoning"));
}

#[test]
fn scrub_internal_control_tokens_preserves_malformed_think_block() {
    let cleaned = scrub_internal_control_tokens(
        "The literal malformed marker <think> has no closing tag in this answer.",
    );

    assert_eq!(
        cleaned,
        "The literal malformed marker <think> has no closing tag in this answer."
    );
}

#[test]
fn scrub_internal_control_tokens_preserves_literal_balanced_think_text() {
    let cleaned = scrub_internal_control_tokens("Use <think>inner</think> in this XML example.");

    assert_eq!(cleaned, "Use <think>inner</think> in this XML example.");
}

#[test]
fn scrub_internal_control_tokens_preserves_literal_leading_balanced_think_text() {
    let cleaned = scrub_internal_control_tokens("<think>inner</think> is an XML example.");

    assert_eq!(cleaned, "<think>inner</think> is an XML example.");
}

#[test]
fn scrub_internal_control_tokens_removes_dsml_block_with_multiple_invokes() {
    let cleaned = scrub_internal_control_tokens(
        "Before\n<｜DSML｜tool_calls>\n<｜DSML｜invoke name=\"read_file\">\n<｜DSML｜parameter name=\"path\" string=\"true\">src/lib.rs</｜DSML｜parameter>\n</｜DSML｜invoke>\n<｜DSML｜invoke name=\"list_files\">\n<｜DSML｜parameter name=\"path\" string=\"true\">src</｜DSML｜parameter>\n</｜DSML｜invoke>\n</｜DSML｜tool_calls>\nAfter",
    );

    assert_eq!(cleaned.trim(), "Before\n\nAfter");
    assert!(!cleaned.contains("read_file"));
    assert!(!cleaned.contains("list_files"));
}

#[test]
fn scrub_internal_control_tokens_removes_dsml_tool_block_without_string_attribute() {
    let input = "Before\n<｜DSML｜tool_calls>\n<｜DSML｜invoke name=\"read_file\">\n<｜DSML｜parameter name=\"options\">{\"path\":\"src/lib.rs\"}</｜DSML｜parameter>\n</｜DSML｜invoke>\n</｜DSML｜tool_calls>\nAfter";
    let cleaned = scrub_internal_control_tokens(input);

    assert_eq!(cleaned.trim(), "Before\n\nAfter");
    assert!(!cleaned.contains("read_file"));
}

#[test]
fn scrub_internal_control_tokens_removes_dsml_tool_block_with_duplicate_parameter_names() {
    let input = "Before\n<｜DSML｜tool_calls>\n<｜DSML｜invoke name=\"read_file\">\n<｜DSML｜parameter name=\"path\" string=\"true\">src/lib.rs</｜DSML｜parameter>\n<｜DSML｜parameter name=\"path\" string=\"true\">src/main.rs</｜DSML｜parameter>\n</｜DSML｜invoke>\n</｜DSML｜tool_calls>\nAfter";
    let cleaned = scrub_internal_control_tokens(input);

    assert_eq!(cleaned.trim(), "Before\n\nAfter");
    assert!(!cleaned.contains("read_file"));
}

#[test]
fn scrub_internal_control_tokens_preserves_literal_tool_call_text() {
    let cleaned =
        scrub_internal_control_tokens("The literal marker `tool_call:` appears in this log line.");

    assert_eq!(
        cleaned,
        "The literal marker `tool_call:` appears in this log line."
    );
}

#[test]
fn scrub_internal_control_tokens_preserves_raw_tool_call_text() {
    let cleaned = scrub_internal_control_tokens(
        "Here is the raw log: `| tool_call: read_file arguments: {\"path\":\"src/lib.rs\"}`",
    );

    assert_eq!(
        cleaned,
        "Here is the raw log: `| tool_call: read_file arguments: {\"path\":\"src/lib.rs\"}`"
    );
}

#[test]
fn scrub_internal_control_tokens_preserves_single_meta_intro_line() {
    let cleaned = scrub_internal_control_tokens(
        "The user asked a good question about plan mode.\nThis answer explains the runtime boundary.",
    );

    assert_eq!(
        cleaned,
        "The user asked a good question about plan mode.\nThis answer explains the runtime boundary."
    );
}

#[test]
fn scrub_internal_control_tokens_drops_orphaned_dsml_payload() {
    let cleaned = scrub_internal_control_tokens(
        "kind: format!(\"unknown_retrieval_{tool_name}\"),\nlabel: format!(\"Unknown Retrieval ({tool_name})\"),\n}\n<｜DSML｜parameter name=\"path\" string=\"true\">src/context/selection.rs</｜DSML｜parameter>\n</｜DSML｜invoke>\n</｜DSML｜tool_calls>",
    );

    assert!(cleaned.trim().is_empty());
}

#[test]
fn scrub_internal_control_tokens_preserves_visible_text_before_orphaned_dsml_tail() {
    let cleaned = scrub_internal_control_tokens(
        "Visible answer.\n<｜DSML｜parameter name=\"path\" string=\"true\">src/context/selection.rs</｜DSML｜parameter>\n</｜DSML｜invoke>\n</｜DSML｜tool_calls>",
    );

    assert_eq!(cleaned, "Visible answer.");
}

#[test]
fn scrub_internal_control_tokens_preserves_literal_dsml_closing_tag_text() {
    let input = "Document `path</|DSML|parameter>` as literal markup.";

    assert_eq!(scrub_internal_control_tokens(input), input);
}

#[test]
fn pending_control_prefix_detects_tags_after_visible_punctuation() {
    assert!(has_pending_internal_control_context("Visible:<agent_"));
    assert!(has_pending_internal_control_context(
        "Visible:<｜DSML｜tool_"
    ));
}

#[test]
fn scrub_internal_control_tokens_preserves_colon_text_before_valid_dsml() {
    let cleaned = scrub_internal_control_tokens(
        "The status is: ok\n<｜DSML｜tool_calls>\n<｜DSML｜invoke name=\"read_file\">\n<｜DSML｜parameter name=\"path\" string=\"true\">Cargo.toml</｜DSML｜parameter>\n</｜DSML｜invoke>\n</｜DSML｜tool_calls>",
    );

    assert_eq!(cleaned.trim(), "The status is: ok");
}

#[test]
fn scrub_internal_control_tokens_preserves_malformed_dsml_remainder() {
    let cleaned = scrub_internal_control_tokens(
        "Before\n<｜DSML｜tool_calls>\n<｜DSML｜invoke name=\"replace\">\nAfter normal text",
    );

    assert!(cleaned.contains("Before"));
    assert!(cleaned.contains("<｜DSML｜tool_calls>"));
    assert!(cleaned.contains("After normal text"));
}

#[test]
fn plan_mode_routes_planning_prose_to_planning_not_exploring() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
    app.set_agent_execution_mode(AgentExecutionMode::Plan);
    app.record_exploration_action("Read crates/instructions/src/workspace.rs");
    app.runtime_phase = RuntimePhase::RunningTool;

    apply_tui_event(
        &mut app,
        TuiEvent::Transcript {
            role: "Agent",
            message: "Based on the inspection of `crates/instructions/src/workspace.rs`, I propose the following plan:<channel|>\n1. Generalize prompt discovery.\n2. Keep the current merge semantics.".into(),
        },
    );

    assert!(app.active_live.exploration_notes.is_empty());
    assert_eq!(
        app.active_live.planning_notes,
        vec![
            "1. Generalize prompt discovery.".to_string(),
            "2. Keep the current merge semantics.".to_string()
        ]
    );
}
