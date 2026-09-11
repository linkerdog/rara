use std::sync::{Arc, Mutex};

use rara_memory::memory_handle::MemoryHandle;
use rara_observability::InferenceTask;
use rara_tools::tool::ToolManager;
use serde_json::json;

use super::support::{SequencedBackend, test_runtime_storage};
use crate::agent::{Agent, AgentExecutionMode, CacheExperimentOptions, ToolCall, ToolSchemaPolicy};
use crate::llm::{
    ContentBlock, LlmBackend, LlmResponse, LlmStreamEvent, LlmTurnMetadata, Message, SummaryPrefix,
    SummaryStrategy,
};

#[derive(Default)]
struct SummaryBackend {
    main_requests: Mutex<Vec<Vec<Message>>>,
    summary_requests: Mutex<Vec<SummaryPrefix>>,
}

fn record_attempt(metadata: &LlmTurnMetadata, result: &anyhow::Result<impl Sized>) {
    if let Some(attempt) = metadata.start_attempt("fixture", "main") {
        attempt.record_final_usage(rara_observability::InferenceTokenUsage {
            input_tokens: 100,
            output_tokens: 10,
            cache_read_tokens: Some(80),
            cache_write_tokens: Some(0),
            cache_write_5m_tokens: Some(0),
            cache_write_1h_tokens: Some(0),
        });
        attempt.finish(result);
    }
}

#[async_trait::async_trait]
impl LlmBackend for SummaryBackend {
    async fn ask(
        &self,
        messages: &[Message],
        _tools: &[serde_json::Value],
    ) -> anyhow::Result<LlmResponse> {
        self.main_requests.lock().unwrap().push(messages.to_vec());
        Ok(LlmResponse {
            content: vec![ContentBlock::Text {
                text: "checked the relevant source".into(),
            }],
            stop_reason: Some("end_turn".into()),
            usage: None,
        })
    }

    async fn ask_streaming_with_context(
        &self,
        messages: &[Message],
        tools: &[serde_json::Value],
        metadata: LlmTurnMetadata,
        _on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> anyhow::Result<LlmResponse> {
        let result = self.ask(messages, tools).await;
        record_attempt(&metadata, &result);
        result
    }

    async fn summarize(&self, _messages: &[Message], _instruction: &str) -> anyhow::Result<String> {
        anyhow::bail!("unexpected auxiliary summary")
    }

    async fn summarize_with_prefix(
        &self,
        messages: &[Message],
        instruction: &str,
        prefix: &SummaryPrefix,
        metadata: LlmTurnMetadata,
    ) -> anyhow::Result<String> {
        prefix.messages_for_summary(messages, instruction)?;
        assert!(!prefix.tools.is_empty());
        self.summary_requests.lock().unwrap().push(prefix.clone());
        let result = Ok("Objective: continue the source review. Evidence: source was checked. Next: verify the result.".into());
        record_attempt(&metadata, &result);
        result
    }
}

#[tokio::test]
async fn task_accounting_includes_boundary_summary_and_cache_rebuild() {
    let (_temp, sessions, workspace, state) = test_runtime_storage();
    let mut tools = ToolManager::new();
    tools.register(Box::<rara_tools::file::ReadFileTool>::default());
    let backend = Arc::new(SummaryBackend::default());
    let mut agent = Agent::new(
        tools,
        backend.clone(),
        Arc::new(MemoryHandle::new(&state.join("memory").to_string_lossy())),
        sessions,
        workspace,
    );
    agent.configure_cache_experiment(CacheExperimentOptions {
        summary: SummaryStrategy::CachedMainModel,
        ..Default::default()
    });
    let accounting = InferenceTask::default();
    for prompt in ["inspect the first source", "inspect the second source"] {
        agent.pending_inference_agent = Some(accounting.start_agent(None));
        agent
            .query_with_mode(prompt.into(), crate::agent::AgentOutputMode::Silent)
            .await
            .unwrap();
    }
    assert!(agent.compact_now_with_reporter(|_| {}).await.unwrap());
    assert_eq!(backend.summary_requests.lock().unwrap().len(), 1);
    agent.pending_inference_agent = Some(accounting.start_agent(None));
    agent
        .query_with_mode(
            "verify the next source".into(),
            crate::agent::AgentOutputMode::Silent,
        )
        .await
        .unwrap();
    let snapshot = accounting.snapshot();
    assert!(snapshot.is_terminal());
    assert_eq!(snapshot.calls.len(), 4);
    assert_eq!(snapshot.attempts.len(), 4);
    assert_eq!(
        snapshot.calls[2].purpose,
        rara_observability::InferencePurpose::Summary
    );
    assert_eq!(
        snapshot.calls[3].purpose,
        rara_observability::InferencePurpose::Main
    );
    let requests = backend.main_requests.lock().unwrap();
    assert_eq!(requests[0][0], requests[2][0]);
    assert_ne!(requests[1][1], requests[2][1]);
}

#[tokio::test]
async fn stable_schemas_do_not_allow_review_writes_or_mode_changes() {
    let (temp, sessions, workspace, state) = test_runtime_storage();
    let mut tools = ToolManager::new();
    tools.register(Box::<rara_tools::file::ReadFileTool>::default());
    tools.register(Box::<rara_tools::file::WriteFileTool>::default());
    tools.register(Box::new(rara_tools::planning::EnterPlanModeTool));
    tools.register(Box::new(rara_tools::planning::ExitPlanModeTool));
    let mut agent = Agent::new(
        tools,
        Arc::new(SequencedBackend::new(vec![])),
        Arc::new(MemoryHandle::new(&state.join("memory").to_string_lossy())),
        sessions,
        workspace,
    );
    agent.configure_cache_experiment(CacheExperimentOptions {
        tool_schemas: ToolSchemaPolicy::SessionStable,
        ..Default::default()
    });
    let execute = agent.visible_tool_schemas();
    agent.execution_mode = AgentExecutionMode::Plan;
    assert_eq!(execute, agent.visible_tool_schemas());
    agent.execution_mode = AgentExecutionMode::Review;
    assert_eq!(execute, agent.visible_tool_schemas());
    let accounting = InferenceTask::default();
    let lease = accounting.start_agent(None);
    agent.inference_context = Some(lease.context());
    let target = temp.path().join("must-not-exist");
    let calls = vec![
        ToolCall {
            id: "write".into(),
            name: "write_file".into(),
            input: json!({"path": target, "content": "forbidden"}),
        },
        ToolCall {
            id: "enter".into(),
            name: "enter_plan_mode".into(),
            input: json!({}),
        },
        ToolCall {
            id: "exit".into(),
            name: "exit_plan_mode".into(),
            input: json!({}),
        },
    ];
    let results = agent.execute_tool_calls(calls, &mut |_| {}).await.unwrap();
    assert_eq!(results.len(), 3);
    assert!(
        results
            .iter()
            .all(|message| message.content[0]["is_error"] == true)
    );
    assert!(!target.exists());
    assert_eq!(agent.execution_mode, AgentExecutionMode::Review);
    assert!(agent.pending_approval.is_none());
    assert!(agent.pending_plan_exit_tool_id.is_none());
    assert_eq!(accounting.snapshot().tool_requests, 3);
    assert_eq!(accounting.snapshot().rejected_tool_requests, 3);
}
