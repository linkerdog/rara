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

#[test]
fn structured_assistant_text_preserves_plan_mode_routing() {
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
        TuiEvent::Runtime(Box::new(crate::runtime_control::RuntimeControlEvent {
            event_id: "event-1".into(),
            provenance: crate::runtime_control::RuntimeProvenance::local_tui("session-1"),
            sequence: 1,
            event: RuntimeEvent::Assistant(crate::runtime_control::AssistantEvent::Text(
                "Based on the inspection of `crates/instructions/src/workspace.rs`, I propose the following plan:<channel|>\n1. Generalize prompt discovery.\n2. Keep the current merge semantics.".into(),
            )),
        })),
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

#[test]
fn agent_dsml_only_message_does_not_enter_transcript() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");

    apply_tui_event(
        &mut app,
        TuiEvent::Transcript {
            role: "Agent",
            message: "<｜DSML｜tool_calls>\n<｜DSML｜invoke name=\"replace\"></｜DSML｜invoke>\n</｜DSML｜tool_calls>".into(),
        },
    );

    assert!(app.active_turn.entries.is_empty());
}

#[test]
fn agent_thinking_delta_updates_live_thinking_without_transcript_entry() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");

    apply_tui_event(
        &mut app,
        runtime_event_from_agent_event(
            AgentEvent::AssistantThinkingDelta("checking relevant files".to_string()),
            RuntimeProvenance::local_tui("session-1"),
        ),
    );

    assert_eq!(app.runtime_phase, RuntimePhase::ProcessingResponse);
    assert_eq!(app.runtime_phase_detail.as_deref(), Some("thinking"));
    assert!(app.active_turn.entries.is_empty());
    let rendered = app
        .agent_thinking_stream_lines()
        .expect("thinking stream")
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("checking relevant files"));
}

#[test]
fn bash_rg_tool_use_is_shown_as_exploration() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");

    apply_tui_event(
        &mut app,
        TuiEvent::Transcript {
            role: "Tool",
            message: "bash rg --files src/tui".into(),
        },
    );
    apply_tui_event(
        &mut app,
        TuiEvent::Transcript {
            role: "Tool",
            message: "bash cd src && rg -n \"render\" tui".into(),
        },
    );

    assert_eq!(
        app.active_live.exploration_actions,
        vec![
            "Find files src/tui".to_string(),
            "Search render src/tui".to_string()
        ]
    );
    assert!(app.active_live.running_actions.is_empty());
}

#[test]
fn formats_apply_patch_tool_use_with_target_files() {
    let rendered = format_apply_patch_use(&json!({
        "patch": "*** Begin Patch\n*** Update File: src/tui/render.rs\n@@\n-old\n+new\n*** Update File: src/tui/runtime/events.rs\n@@\n-old\n+new\n*** End Patch"
    }));
    assert_eq!(
        rendered,
        "apply_patch src/tui/render.rs, src/tui/runtime/events.rs"
    );
}

#[test]
fn formats_apply_patch_tool_result_as_diff_summary() {
    let rendered = format_apply_patch_result(&json!({
        "status": "ok",
        "files_changed": 2,
        "line_delta": { "added": 12, "removed": 3 },
        "updated_files": ["src/tui/render.rs"],
        "created_files": ["src/tui/render/bottom_pane.rs"],
        "summary": [
            "updated src/tui/render.rs",
            "created src/tui/render/bottom_pane.rs"
        ],
        "diff_preview": "*** Begin Patch\n*** Update File: src/tui/render.rs\n@@\n-old\n+new\n*** End Patch"
    }));

    assert!(rendered.contains("apply_patch ok 2 file(s) (+12 -3)"));
    assert!(rendered.contains("updated: src/tui/render.rs"));
    assert!(rendered.contains("created: src/tui/render/bottom_pane.rs"));
    assert!(rendered.contains("changes:"));
    assert!(rendered.contains("diff:"));
    assert!(rendered.contains("-old"));
    assert!(rendered.contains("+new"));
}

#[test]
fn formats_replace_lines_tool_use_as_file_range() {
    let rendered = format_tool_use(
        "replace_lines",
        &json!({
            "path": "src/context/assembler.rs",
            "start_line": 426,
            "end_line": 1263,
            "new_string": ""
        }),
    );

    assert_eq!(rendered, "replace_lines src/context/assembler.rs:426-1263");
}

#[test]
fn formats_spawn_agent_tool_use_without_dumping_instruction_json() {
    let rendered = format_tool_use(
        "spawn_agent",
        &json!({
            "name": "fix-assembler",
            "instruction": "Fix src/context/assembler.rs by removing the orphaned code block between the two cfg(test) markers.\nRead the file in small chunks and do not use a giant replace old_string payload."
        }),
    );

    assert!(rendered.starts_with("spawn_agent fix-assembler: Fix src/context/assembler.rs"));
    assert!(rendered.ends_with('…'));
    assert!(!rendered.contains("\"instruction\""));
    assert!(!rendered.contains('\n'));
}

#[test]
fn formats_spawn_agent_tool_result_with_agent_name() {
    let rendered = format_tool_result(
        "spawn_agent",
        &json!({
            "name": "fix-assembler",
            "status": "done",
            "summary": "Removed the orphaned code block."
        })
        .to_string(),
    );

    assert_eq!(
        rendered,
        "spawn_agent fix-assembler: Removed the orphaned code block."
    );
}

#[test]
fn formats_subagent_tool_result_with_returned_ids() {
    let rendered = format_tool_result(
        "spawn_agent",
        &json!({
            "agent_id": "fix-assembler-1",
            "session_id": "child-session-1",
            "name": "fix-assembler",
            "status": "done",
            "summary": "Removed the orphaned code block.",
            "persistence_error": "sidechain write failed"
        })
        .to_string(),
    );

    assert!(rendered.contains("agent_id: fix-assembler-1"));
    assert!(rendered.contains("session_id: child-session-1"));
    assert!(rendered.contains("persistence_error: sidechain write failed"));
}

#[test]
fn formats_lsp_diagnostics_tool_result_as_parseable_payload() {
    let rendered = format_tool_result(
        "lsp_diagnostics",
        &serde_json::to_string_pretty(&json!({
            "file": "src/main.rs",
            "diagnostics": [{
                "file": "src/main.rs",
                "line": 4,
                "column": 8,
                "severity": "Error",
                "message": "cannot find value `x` in this scope",
                "code": "E0425"
            }],
            "status": {
                "diagnostic_count": 1,
                "servers": [{ "running": true }]
            }
        }))
        .unwrap(),
    );

    assert!(rendered.starts_with("lsp_diagnostics\n{"));
    assert!(rendered.contains("\"diagnostics\""));
    assert!(rendered.contains("\"E0425\""));
}

#[test]
fn formats_replace_lines_tool_result_as_edit_summary() {
    let rendered = format_tool_result(
        "replace_lines",
        &json!({
            "status": "ok",
            "path": "src/context/assembler.rs",
            "start_line": 426,
            "end_line": 1263,
            "removed_lines": 838,
            "inserted_lines": 0,
            "line_delta": -838
        })
        .to_string(),
    );

    assert_eq!(
        rendered,
        "~ replace_lines src/context/assembler.rs:426-1263  -838 lines  +0 lines  (Δ -838)"
    );
}

#[test]
fn formats_bash_tool_result_with_output_tail() {
    let rendered = format_tool_result(
        "bash",
        &json!({
            "exit_code": 0,
            "stdout": "line 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\n",
            "stderr": "warn 1\nwarn 2\n"
        })
        .to_string(),
    );

    assert!(rendered.contains("bash finished with exit code 0"));
    assert!(rendered.contains("stdout:"));
    assert!(rendered.contains("stderr:"));
    assert!(rendered.contains("line 7"));
    assert!(rendered.contains("line 6"));
    assert!(!rendered.contains("line 1"));
}

#[test]
fn formats_live_bash_tool_result_with_output_tail() {
    let rendered = format_tool_result(
        "bash",
        &json!({
            "exit_code": 0,
            "stdout": "line 1\nline 2\n",
            "stderr": "",
            "live_streamed": true
        })
        .to_string(),
    );

    assert!(rendered.contains("bash finished with exit code 0"));
    assert!(rendered.contains("output streamed above"));
    assert!(!rendered.contains("stdout:"));
    assert!(!rendered.contains("stderr:"));
    assert!(rendered.contains("line 2"));
}

#[test]
fn formats_stderr_only_bash_tool_result_with_stream_label() {
    let rendered = format_tool_result(
        "bash",
        &json!({
            "exit_code": 1,
            "stdout": "",
            "stderr": "warn 1\nwarn 2\n"
        })
        .to_string(),
    );

    assert!(rendered.contains("bash failed with exit code 1"));
    assert!(!rendered.contains("stdout:"));
    assert!(rendered.contains("stderr:"));
    assert!(rendered.contains("warn 2"));
}

#[test]
fn formats_signaled_bash_tool_result_without_unknown_status() {
    let rendered = format_tool_result(
        "bash",
        &json!({
            "exit_code": null,
            "termination": {
                "kind": "signal",
                "signal": 6,
                "name": "SIGABRT"
            },
            "sandbox_failure": {
                "kind": "sandboxed_process_signaled",
                "backend": "macos-seatbelt"
            },
            "stdout": "",
            "stderr": ""
        })
        .to_string(),
    );

    assert!(rendered.contains("terminated by SIGABRT (signal 6)"));
    assert!(rendered.contains("Sandbox: process signaled (macos-seatbelt)"));
    assert!(!rendered.contains("unknown exit status"));
}

#[test]
fn formats_background_bash_start_as_task_summary() {
    let rendered = format_tool_result(
        "bash",
        &json!({
            "exit_code": null,
            "live_streamed": false,
            "background_task_id": "bash-123",
            "output_path": "/tmp/rara/background-tasks/bash-123.log",
            "status": "running"
        })
        .to_string(),
    );

    assert_eq!(
        rendered,
        "background task bash-123 running\noutput: /tmp/rara/background-tasks/bash-123.log"
    );
}

#[test]
fn formats_pty_result_with_sanitized_output_tail() {
    let rendered = format_tool_result(
        "pty_status",
        &json!({
            "session_id": "pty-123",
            "command": "npm run dev",
            "status": "running",
            "output": "\u{1b}[32mready\u{1b}[0m\r\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\n"
        })
        .to_string(),
    );

    assert!(rendered.starts_with("pty pty-123 running: npm run dev"));
    assert!(rendered.contains("output:"));
    assert!(rendered.contains("line 7"));
    assert!(rendered.contains("line 2"));
    assert!(!rendered.contains("ready"));
    assert!(!rendered.contains('\u{1b}'));
}

#[test]
fn formats_background_task_status_with_output_tail() {
    let rendered = format_tool_result(
        "background_task_status",
        &json!({
            "task_id": "bash-123",
            "command": "cargo test",
            "status": "completed",
            "exit_code": 0,
            "output": "build\nrunning\nok\n"
        })
        .to_string(),
    );

    assert_eq!(
        rendered,
        "background task bash-123 completed: cargo test\nexit_code: 0\noutput:\nbuild\nrunning\nok"
    );
}

#[test]
fn formats_terminal_list_and_stop_results() {
    let listed = format_tool_result(
        "pty_list",
        &json!({
            "sessions": [
                {
                    "session_id": "pty-1",
                    "command": "python repl.py",
                    "status": "running"
                }
            ]
        })
        .to_string(),
    );
    assert_eq!(
        listed,
        "pty sessions: 1\n  pty pty-1 running: python repl.py"
    );

    let stopped = format_tool_result(
        "background_task_stop",
        &json!({
            "stopped": [
                {
                    "id": "bash-1",
                    "command": "sleep 10",
                    "status": "killed"
                }
            ]
        })
        .to_string(),
    );
    assert_eq!(
        stopped,
        "background task stopped: 1\n  background task bash-1 killed: sleep 10"
    );
}

#[test]
fn formats_terminal_tool_use_without_dumping_json() {
    let rendered = format_tool_use(
        "pty_write",
        &json!({
            "session_id": "pty-123",
            "input": "hello\n"
        }),
    );

    assert_eq!(rendered, "pty_write pty-123: hello\\n");
}

#[test]
fn converts_terminal_tool_result_to_typed_event() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
    let event = runtime_event_from_agent_event(
        AgentEvent::ToolResult {
            name: "pty_status".to_string(),
            content: json!({
                "session_id": "pty-123",
                "command": "cargo test",
                "status": "completed",
                "output": "ok\n"
            })
            .to_string(),
            is_error: false,
        },
        RuntimeProvenance::local_tui("session-1"),
    );

    apply_tui_event(&mut app, event);
    match app
        .active_turn
        .entries
        .last()
        .and_then(|entry| entry.payload.as_ref())
    {
        Some(crate::tui::state::TranscriptEntryPayload::Terminal(TerminalEvent::End(command))) => {
            assert_eq!(command.target, TerminalTarget::Pty);
            assert_eq!(command.id.as_deref(), Some("pty-123"));
            assert_eq!(command.status, "completed");
            assert_eq!(command.command.as_deref(), Some("cargo test"));
            assert_eq!(command.output, vec!["ok".to_string()]);
        }
        _ => panic!("unexpected event"),
    }
}

#[test]
fn todo_write_events_render_as_compact_todo_transcript() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
    let state = crate::todo::normalize_todo_write_input(&json!({
        "todos": [
            {"content": "Implement todo runtime", "status": "in_progress"},
            {"content": "Run tests", "status": "pending"}
        ]
    }))
    .expect("todo state");

    apply_tui_event(
        &mut app,
        runtime_event_from_agent_event(
            AgentEvent::TodoUpdated(state),
            RuntimeProvenance::local_tui("session-1"),
        ),
    );
    assert_eq!(app.snapshot.todo.summary.total, 2);
    assert_eq!(app.snapshot.todo.summary.in_progress, 1);
}

#[test]
fn mcp_status_update_events_stay_off_tui_transcript() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");
    apply_tui_event(
        &mut app,
        runtime_event_from_agent_event(
            AgentEvent::McpStatusUpdated(crate::mcp_status::McpStatusSnapshot { servers: vec![] }),
            RuntimeProvenance::local_tui("session-1"),
        ),
    );
    assert!(app.active_turn.entries.is_empty());
}

#[test]
fn applies_terminal_begin_event_as_running_action() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");

    let event = runtime_event_from_agent_event(
        AgentEvent::ToolUse {
            name: "bash".to_string(),
            input: json!({
                "command": "cargo test",
                "run_in_background": true
            }),
        },
        RuntimeProvenance::local_tui("session-1"),
    );

    apply_tui_event(&mut app, event);

    assert_eq!(
        app.active_live.running_actions,
        vec!["Run cargo test".to_string()]
    );
    assert_eq!(app.active_turn.entries.len(), 1);
    assert_eq!(app.active_turn.entries[0].role, "Terminal Event");
    match app.active_turn.entries[0].payload.as_ref() {
        Some(TranscriptEntryPayload::Terminal(TerminalEvent::Begin(command))) => {
            assert_eq!(command.target, TerminalTarget::BackgroundTask);
            assert_eq!(command.command.as_deref(), Some("cargo test"));
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn formats_generic_tool_result_without_preview_available_marker() {
    let rendered = format_tool_result(
        "bash",
        "Tool bash completed with exit_code, live_streamed, stderr, stdout.\nline 1\nline 2",
    );

    assert!(rendered.contains("bash: bash finished"));
    assert!(rendered.contains("line 1"));
    assert!(rendered.contains("line 2"));
    assert!(!rendered.contains("preview available"));
}

#[test]
fn formats_persisted_bash_result_without_generic_prefix() {
    let rendered = format_tool_result(
        "bash",
        "finished with exit code 0\nDuration: 10 ms\nOutput:\nline 1\nline 2\n\n[tool_result truncated]\nfull result: /tmp/rara/tool-results/tool-1.json",
    );

    assert!(rendered.starts_with("bash: finished with exit code 0"));
    assert!(rendered.contains("Output:\nline 1\nline 2"));
    assert!(rendered.contains("full result: /tmp/rara/tool-results/tool-1.json"));
    assert!(!rendered.contains("bash: bash finished"));
    assert!(!rendered.contains("full result stored on disk"));
}

#[test]
fn formats_tool_progress_with_stream_label() {
    let rendered = format_tool_progress("bash", ToolOutputStream::Stderr, "warn 1\nwarn 2\n");
    assert_eq!(rendered, "bash stderr:\nwarn 1\nwarn 2\n");
}

#[test]
fn skips_tool_progress_when_stderr_has_no_visible_output() {
    let rendered = format_tool_progress(
        "background task",
        ToolOutputStream::Stderr,
        "\u{1b}[2K\r\n   \n",
    );

    assert_eq!(rendered, "");
}

#[test]
fn runtime_device_code_messages_update_prompt_and_polling_phases() {
    let temp = tempdir().expect("tempdir");
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("app");

    apply_tui_event(
        &mut app,
        TuiEvent::Transcript {
            role: "Runtime",
            message: "Open this URL in a browser and enter the one-time code:\nhttps://example.test\n\nCode: ABCD".into(),
        },
    );
    assert_eq!(app.runtime_phase, RuntimePhase::OAuthDeviceCodePrompt);
    let prompt_entry = app
        .active_turn
        .entries
        .last()
        .expect("persisted oauth prompt entry");
    assert_eq!(prompt_entry.role, "System");
    assert!(prompt_entry.message.contains("https://example.test"));
    assert!(prompt_entry.message.contains("Code: ABCD"));

    apply_tui_event(
        &mut app,
        TuiEvent::Transcript {
            role: "Runtime",
            message: "Waiting for device-code confirmation.".into(),
        },
    );
    assert_eq!(app.runtime_phase, RuntimePhase::OAuthPollingDeviceCode);
}

#[test]
fn detects_persistent_oauth_prompt_messages() {
    assert!(is_oauth_prompt_message(
        "Open this URL in a browser and enter the one-time code:\nhttps://example.test\n\nCode: ABCD"
    ));
    assert!(is_oauth_prompt_message(
        "Starting Codex browser login.\nOpen this URL if the browser does not launch automatically:\nhttps://example.test"
    ));
    assert!(!is_oauth_prompt_message(
        "Waiting for device-code confirmation."
    ));
}
