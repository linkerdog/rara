use std::sync::Arc;

use serde_json::json;

use super::support::{SequencedBackend, test_runtime_storage};
use crate::agent::{Agent, AgentExecutionMode, Message, PlanStep, PlanStepStatus};
use crate::llm::{ContentBlock, LlmResponse};
use crate::memory_store::{MemoryLabel, MemoryScope, MemorySource, MemoryStore, NewMemoryRecord};
use crate::prompt::PromptRuntimeConfig;
use crate::tool::ToolManager;
use crate::vectordb::VectorDB;

#[test]
fn shared_runtime_context_collects_prompt_plan_and_compaction_state() {
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    std::fs::write(
        rara_dir.join("memory.md"),
        "# Team Notes\n\nPrefer the shared bootstrap path.\n",
    )
    .expect("write workspace memory");
    let backend = Arc::new(SequencedBackend::new(vec![LlmResponse {
        content: vec![ContentBlock::Text {
            text: "ok".to_string(),
        }],
        stop_reason: Some("end_turn".to_string()),
        usage: None,
    }]));

    let mut agent = Agent::new(
        ToolManager::new(),
        backend,
        Arc::new(VectorDB::new(
            &rara_dir.join("lancedb").display().to_string(),
        )),
        session_manager,
        workspace,
    );
    agent.set_prompt_config(PromptRuntimeConfig {
        append_system_prompt: Some("appendix".to_string()),
        warnings: vec!["missing prompt file".to_string()],
        ..PromptRuntimeConfig::default()
    });
    agent.execution_mode = AgentExecutionMode::Plan;
    agent.current_plan = vec![
        PlanStep {
            step: "inspect auth flow".to_string(),
            status: PlanStepStatus::Completed,
        },
        PlanStep {
            step: "replace bootstrap path".to_string(),
            status: PlanStepStatus::Pending,
        },
    ];
    agent.plan_explanation = Some("Prefer one shared bootstrap path.".to_string());
    agent.total_input_tokens = 11;
    agent.total_output_tokens = 7;
    agent.total_cache_hit_tokens = 5;
    agent.total_cache_miss_tokens = 3;
    agent.compact_state.estimated_history_tokens = 1234;
    agent.compact_state.context_window_tokens = Some(8192);
    agent.compact_state.compact_threshold_tokens = 7000;
    agent.compact_state.reserved_output_tokens = 1024;
    agent.compact_state.compaction_count = 2;
    agent.compact_state.last_compaction_before_tokens = Some(5000);
    agent.compact_state.last_compaction_after_tokens = Some(2100);
    agent.compact_state.last_compaction_recent_files = vec![
        "src/main.rs".to_string(),
        "src/runtime_context.rs".to_string(),
    ];
    agent.compact_state.last_compaction_boundary = Some(crate::agent::CompactBoundaryMetadata {
        version: 3,
        before_tokens: 5000,
        recent_file_count: 2,
    });
    agent.history.push(Message {
        role: "user".to_string(),
        content: json!([{"type":"text","text":"hello"}]),
    });
    agent.history.push(Message {
        role: "system".to_string(),
        content: json!([{
            "type": "compact_boundary",
            "version": 3,
            "before_tokens": 5000,
            "recent_file_count": 2
        }]),
    });
    agent.history.push(Message {
        role: "system".to_string(),
        content: json!([{
            "type": "compacted_summary",
            "text": "User Intent\n- finish the refactor"
        }]),
    });
    agent.history.push(Message {
        role: "system".to_string(),
        content: json!([{
            "type": "recent_files",
            "files": [
                "src/main.rs",
                "src/runtime_context.rs"
            ]
        }]),
    });
    agent.history.push(Message {
        role: "system".to_string(),
        content: json!([{
            "type": "recent_file_excerpts",
            "files": [{
                "path": "src/main.rs",
                "line_start": 1,
                "line_end": 3,
                "snippet": "code"
            }]
        }]),
    });
    agent.history.push(Message {
        role: "assistant".to_string(),
        content: json!([
            {
                "type": "tool_use",
                "id": "tool-retrieve-1",
                "name": "retrieve_experience",
                "input": { "query": "bootstrap contract" }
            },
            {
                "type": "tool_use",
                "id": "tool-retrieve-2",
                "name": "retrieve_session_context",
                "input": { "query": "previous auth flow" }
            }
        ]),
    });
    agent.history.push(Message {
        role: "user".to_string(),
        content: json!([
            {
                "type": "tool_result",
                "tool_use_id": "tool-retrieve-1",
                "content": "Tool retrieve_experience completed with relevant_experiences.\nPayload:\n{\n  \"relevant_experiences\": [\n    \"Prefer one shared bootstrap path.\",\n    \"Keep session restore aligned with direct execution.\"\n  ]\n}"
            },
            {
                "type": "tool_result",
                "tool_use_id": "tool-retrieve-2",
                "content": "Tool retrieve_session_context completed with status, summary.\nPayload:\n{\n  \"status\": \"ok\",\n  \"summary\": \"Auth picker already moved behind the shared runtime bootstrap.\"\n}"
            }
        ]),
    });

    let runtime = agent.shared_runtime_context();

    assert_eq!(runtime.history_len, 7);
    assert_eq!(runtime.total_input_tokens, 11);
    assert_eq!(runtime.total_output_tokens, 7);
    assert_eq!(runtime.total_cache_hit_tokens, 5);
    assert_eq!(runtime.total_cache_miss_tokens, 3);
    assert_eq!(runtime.prompt.base_prompt_kind, "default");
    assert!(
        runtime
            .prompt
            .section_keys
            .contains(&"append_system_prompt".to_string())
    );
    assert_eq!(
        runtime.prompt.source_entries.len(),
        runtime.prompt.source_status_lines.len()
    );
    assert_eq!(runtime.prompt.source_entries[0].order, 1);
    assert!(!runtime.prompt.source_entries[0].inclusion_reason.is_empty());
    assert!(
        runtime
            .prompt
            .warnings
            .contains(&"missing prompt file".to_string())
    );
    assert_eq!(runtime.plan.execution_mode, "plan");
    assert_eq!(
        runtime.plan.steps,
        vec![
            ("completed".to_string(), "inspect auth flow".to_string()),
            ("pending".to_string(), "replace bootstrap path".to_string()),
        ]
    );
    assert_eq!(
        runtime.plan.explanation.as_deref(),
        Some("Prefer one shared bootstrap path.")
    );
    assert_eq!(runtime.compaction.estimated_history_tokens, 1234);
    assert_eq!(runtime.compaction.context_window_tokens, Some(8192));
    assert_eq!(runtime.compaction.last_compaction_boundary_version, Some(3));
    assert_eq!(
        runtime.compaction.last_compaction_recent_files,
        vec![
            "src/main.rs".to_string(),
            "src/runtime_context.rs".to_string()
        ]
    );
    assert_eq!(runtime.compaction.source_entries.len(), 4);
    assert_eq!(
        runtime.compaction.source_entries[0].kind,
        "compact_boundary"
    );
    assert_eq!(
        runtime.compaction.source_entries[0].source_descriptor,
        "history.compaction.boundary"
    );
    assert_eq!(
        runtime.compaction.source_entries[1].kind,
        "compacted_summary"
    );
    assert_eq!(
        runtime.compaction.source_entries[1].source_descriptor,
        "history.compaction.summary"
    );
    assert_eq!(runtime.compaction.source_entries[2].kind, "recent_files");
    assert_eq!(
        runtime.compaction.source_entries[3].kind,
        "recent_file_excerpts"
    );
    assert_eq!(runtime.retrieval.entries.len(), 4);
    assert_eq!(runtime.retrieval.entries[0].kind, "workspace_memory");
    assert_eq!(runtime.retrieval.entries[0].status, "active");
    assert_eq!(runtime.retrieval.entries[1].kind, "thread_history");
    assert_eq!(runtime.retrieval.entries[1].status, "available");
    assert_eq!(runtime.retrieval.entries[2].kind, "vector_memory");
    assert_eq!(runtime.retrieval.entries[2].status, "available");
    assert_eq!(runtime.retrieval.entries[3].kind, "mcp_resource");
    assert_eq!(runtime.retrieval.entries[3].status, "missing");
    assert_eq!(runtime.retrieval.memory_selection.selected_items.len(), 9);
    assert_eq!(
        runtime.retrieval.memory_selection.selected_items[0].kind,
        "workspace_memory"
    );
    assert!(
        runtime.retrieval.memory_selection.selected_items[0]
            .detail
            .contains(".rara/memory.md")
    );
    assert!(
        runtime.retrieval.memory_selection.selected_items[0]
            .detail
            .contains("2 non-empty lines")
    );
    assert_eq!(
        runtime.retrieval.memory_selection.selected_items[1].kind,
        "compacted_summary"
    );
    assert_eq!(
        runtime.retrieval.memory_selection.selected_items[2].kind,
        "recent_files"
    );
    assert_eq!(
        runtime.retrieval.memory_selection.selected_items[3].kind,
        "recent_file_excerpts"
    );
    assert_eq!(
        runtime.retrieval.memory_selection.selected_items[4].kind,
        "plan_explanation"
    );
    assert_eq!(
        runtime.retrieval.memory_selection.selected_items[5].kind,
        "plan_steps"
    );
    assert_eq!(
        runtime.retrieval.memory_selection.selected_items[6].kind,
        "latest_user_request"
    );
    assert_eq!(
        runtime.retrieval.memory_selection.selected_items[7].kind,
        "tool_result"
    );
    assert_eq!(
        runtime.retrieval.memory_selection.selected_items[8].kind,
        "tool_result"
    );
    assert!(
        runtime
            .retrieval
            .memory_selection
            .selected_items
            .iter()
            .all(|item| !matches!(
                item.kind.as_str(),
                "retrieved_workspace_memory" | "retrieved_thread_context"
            ))
    );
    assert_eq!(runtime.retrieval.memory_selection.available_items.len(), 2);
    assert_eq!(
        runtime.retrieval.memory_selection.available_items[0].kind,
        "thread_history"
    );
    assert_eq!(
        runtime.retrieval.memory_selection.available_items[1].kind,
        "vector_memory"
    );
    assert_eq!(runtime.retrieval.memory_selection.dropped_items.len(), 2);
    assert_eq!(
        runtime.retrieval.memory_selection.dropped_items[0].kind,
        "retrieved_workspace_memory"
    );
    assert!(
        runtime.retrieval.memory_selection.dropped_items[0]
            .detail
            .contains("recalled=2 item(s)")
    );
    assert_eq!(
        runtime.retrieval.memory_selection.dropped_items[1].kind,
        "retrieved_thread_context"
    );
    assert!(
        runtime.retrieval.memory_selection.dropped_items[1]
            .detail
            .contains("Auth picker already moved behind the shared runtime bootstrap.")
    );
    assert!(
        runtime
            .retrieval
            .memory_selection
            .selection_budget_tokens
            .is_some()
    );
    assert!(runtime.budget.stable_instructions_budget > 0);
    assert!(runtime.budget.active_turn_budget > 0);
    assert!(
        runtime
            .assembly
            .entries
            .iter()
            .any(|entry| entry.layer == "stable_instructions" && entry.injected)
    );
    assert!(
        runtime
            .assembly
            .entries
            .iter()
            .any(|entry| entry.layer == "compacted_history"
                && entry.injected
                && entry.source_path.as_deref() == Some("history.compaction.summary"))
    );
    assert!(
        runtime
            .assembly
            .entries
            .iter()
            .any(|entry| entry.layer == "retrieval_ready" && !entry.injected)
    );
}

#[test]
fn assemble_turn_context_matches_prompt_and_runtime_views() {
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    std::fs::write(
        rara_dir.join("memory.md"),
        "# Team Notes\n\nPrefer the shared bootstrap path.\n",
    )
    .expect("write workspace memory");
    let backend = Arc::new(SequencedBackend::new(vec![LlmResponse {
        content: vec![ContentBlock::Text {
            text: "ok".to_string(),
        }],
        stop_reason: Some("end_turn".to_string()),
        usage: None,
    }]));

    let mut agent = Agent::new(
        ToolManager::new(),
        backend,
        Arc::new(VectorDB::new(
            &rara_dir.join("lancedb").display().to_string(),
        )),
        session_manager,
        workspace,
    );
    agent.set_prompt_config(PromptRuntimeConfig {
        append_system_prompt: Some("appendix".to_string()),
        warnings: vec!["missing prompt file".to_string()],
        ..PromptRuntimeConfig::default()
    });
    agent.execution_mode = AgentExecutionMode::Plan;
    agent.current_plan = vec![PlanStep {
        step: "inspect auth flow".to_string(),
        status: PlanStepStatus::Pending,
    }];
    agent.plan_explanation = Some("Prefer one shared bootstrap path.".to_string());
    agent.history.push(Message {
        role: "user".to_string(),
        content: json!([{"type":"text","text":"hello"}]),
    });

    let assembled = agent.assemble_turn_context();

    assert_eq!(
        assembled.prompt.system_prompt(),
        agent.build_system_prompt()
    );
    assert_eq!(assembled.runtime, agent.shared_runtime_context());
    assert_eq!(
        assembled.runtime.prompt.append_system_prompt.as_deref(),
        Some("appendix")
    );
    assert_eq!(assembled.runtime.plan.execution_mode, "plan");
}

#[tokio::test]
async fn query_injects_selected_memory_context_without_persisting_it_to_history() {
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let backend = Arc::new(SequencedBackend::new(vec![LlmResponse {
        content: vec![ContentBlock::Text {
            text: "ok".to_string(),
        }],
        stop_reason: Some("end_turn".to_string()),
        usage: None,
    }]));
    let vdb = Arc::new(VectorDB::new(
        &rara_dir.join("lancedb").display().to_string(),
    ));
    let store = MemoryStore::new(backend.clone(), vdb.clone());
    store
        .insert(NewMemoryRecord {
            title: Some("Reference project path".to_string()),
            content: "Reference project source lives at /Users/example/reference-project."
                .to_string(),
            labels: vec![MemoryLabel::Fact],
            importance: 0.9,
            pinned: false,
            source: MemorySource::UserCreated,
            scope: MemoryScope::Workspace,
            session_id: None,
            thread_id: None,
            source_span: None,
        })
        .await
        .expect("insert memory");

    let mut agent = Agent::new(
        ToolManager::new(),
        backend.clone(),
        vdb,
        session_manager,
        workspace,
    );

    agent
        .query_with_mode(
            "Where is the reference project source path?".to_string(),
            crate::agent::AgentOutputMode::Silent,
        )
        .await
        .expect("query");

    let observed = backend.observed_messages();
    let first_turn = observed.first().expect("model call");
    assert_eq!(first_turn[0].role, "system");
    assert!(
        !first_turn
            .windows(2)
            .any(|pair| pair[0].role == "user" && pair[1].role == "user")
    );
    let user_index = first_turn
        .iter()
        .position(|message| {
            message
                .content
                .to_string()
                .contains("Where is the reference project source path?")
        })
        .expect("user request");
    let user_message = &first_turn[user_index];
    assert_eq!(user_message.role, "user");
    let user_content = user_message.content.to_string();
    let memory_position = user_content
        .find("<rara_internal_history_context>")
        .expect("memory context");
    let request_position = user_content
        .find("Where is the reference project source path?")
        .expect("user request");
    assert!(memory_position < request_position);
    assert!(user_content.contains("reference-project"));
    assert!(!first_turn.iter().enumerate().any(|(idx, message)| {
        idx != user_index
            && message
                .content
                .to_string()
                .contains("<rara_internal_history_context>")
    }));
    assert!(!agent.history.iter().any(|message| {
        message
            .content
            .to_string()
            .contains("<rara_internal_history_context>")
    }));
    assert!(
        agent
            .shared_runtime_context()
            .retrieval
            .memory_selection
            .selected_items
            .iter()
            .any(|item| item.kind == crate::context::RETRIEVED_WORKSPACE_MEMORY_KIND)
    );
}

#[test]
fn memory_context_prepends_to_existing_user_message_without_adding_user_turn() {
    let mut messages = vec![
        Message {
            role: "system".to_string(),
            content: json!("system"),
        },
        Message {
            role: "user".to_string(),
            content: json!([{"type":"text","text":"current request"}]),
        },
    ];

    Agent::prepend_memory_context_to_latest_user_message(
        &mut messages,
        "<rara_internal_history_context>\nrecall\n</rara_internal_history_context>".to_string(),
    );

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].role, "user");
    let text = messages[1].content.to_string();
    assert!(text.contains("<rara_internal_history_context>"));
    assert!(text.find("recall").expect("recall") < text.find("current request").expect("request"));
    assert!(
        !messages
            .windows(2)
            .any(|pair| pair[0].role == "user" && pair[1].role == "user")
    );
}

#[test]
fn memory_context_noops_without_user_text_request() {
    let mut messages = vec![
        Message {
            role: "system".to_string(),
            content: json!("system"),
        },
        Message {
            role: "assistant".to_string(),
            content: json!([{"type":"tool_use","id":"tool-1","name":"list_files","input":{}}]),
        },
        Message {
            role: "user".to_string(),
            content: json!([{"type":"tool_result","tool_use_id":"tool-1","content":"ok"}]),
        },
    ];

    Agent::prepend_memory_context_to_latest_user_message(
        &mut messages,
        "<rara_internal_history_context>\nrecall\n</rara_internal_history_context>".to_string(),
    );

    assert_eq!(messages.len(), 3);
    assert!(
        !messages
            .iter()
            .any(|message| message.content.to_string().contains("recall"))
    );
}

#[test]
fn memory_context_skips_tool_result_user_messages() {
    let mut messages = vec![
        Message {
            role: "system".to_string(),
            content: json!("system"),
        },
        Message {
            role: "user".to_string(),
            content: json!([{"type":"text","text":"current request"}]),
        },
        Message {
            role: "assistant".to_string(),
            content: json!([{"type":"tool_use","id":"tool-1","name":"list_files","input":{}}]),
        },
        Message {
            role: "user".to_string(),
            content: json!([{"type":"tool_result","tool_use_id":"tool-1","content":"ok"}]),
        },
    ];

    Agent::prepend_memory_context_to_latest_user_message(
        &mut messages,
        "<rara_internal_history_context>\nrecall\n</rara_internal_history_context>".to_string(),
    );

    assert!(messages[1].content.to_string().contains("recall"));
    assert!(!messages[3].content.to_string().contains("recall"));
}
