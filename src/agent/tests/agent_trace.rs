use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use rara_agent_trace::{
    AgentTraceEvent, AgentTraceRecorder, CacheUsage, TraceModelStatus, TraceRecord,
    TraceTurnOutcome,
};
use rara_memory::memory_handle::MemoryHandle;
use rara_tools::tool::ToolManager;

use super::support::{SequencedBackend, test_runtime_storage};
use crate::agent::{Agent, AgentOutputMode, Message};
use crate::llm::{ContentBlock, LlmBackend, LlmResponse, TokenUsage};

#[tokio::test]
async fn trace_records_agent_turn_without_prompt_or_response_content() -> Result<()> {
    let backend = Arc::new(
        SequencedBackend::new(vec![LlmResponse {
            content: vec![ContentBlock::Text {
                text: "trace-secret-response".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage {
                input_tokens: 120,
                output_tokens: 24,
                cache_hit_tokens: 80,
                cache_miss_tokens: 40,
            }),
        }])
        .with_model_label("trace-model"),
    );
    let mut fixture = TraceFixture::new(backend)?;

    fixture
        .agent
        .query_with_mode("trace-secret-prompt".to_string(), AgentOutputMode::Silent)
        .await?;

    let raw_trace = fs::read_to_string(&fixture.events_path)?;
    assert!(!raw_trace.contains("trace-secret-prompt"));
    assert!(!raw_trace.contains("trace-secret-response"));
    let events = raw_trace
        .lines()
        .map(serde_json::from_str::<TraceRecord>)
        .collect::<Result<Vec<_>, _>>()?;

    assert!(
        events
            .iter()
            .any(|record| { matches!(record.event, AgentTraceEvent::TurnStarted(_)) })
    );
    assert!(
        events
            .iter()
            .any(|record| { matches!(record.event, AgentTraceEvent::ContextAssembled(_)) })
    );
    assert!(
        events
            .iter()
            .any(|record| { matches!(record.event, AgentTraceEvent::AgentStepUpdated(_)) })
    );
    let model = events
        .iter()
        .find_map(|record| match &record.event {
            AgentTraceEvent::ModelFinished(model) => Some(model),
            _ => None,
        })
        .ok_or_else(|| anyhow!("missing model completion trace event"))?;
    assert_eq!(model.model, "trace-model");
    assert_eq!(model.status, TraceModelStatus::Succeeded);
    assert_eq!(
        model.usage.as_ref().map(|usage| usage.input_tokens),
        Some(120)
    );
    assert_eq!(
        model.usage.as_ref().and_then(|usage| usage.cache),
        Some(CacheUsage {
            hit_tokens: 80,
            miss_tokens: 40,
        })
    );
    let turn_finished = events
        .iter()
        .find_map(|record| match &record.event {
            AgentTraceEvent::TurnFinished(finished) => Some(finished),
            _ => None,
        })
        .ok_or_else(|| anyhow!("missing turn completion trace event"))?;
    assert_eq!(turn_finished.outcome, TraceTurnOutcome::Succeeded);
    assert!(
        events
            .iter()
            .all(|record| record.turn_id.as_deref() == Some("turn-42"))
    );

    Ok(())
}

struct TraceFixture {
    _temp: tempfile::TempDir,
    agent: Agent,
    events_path: PathBuf,
}

impl TraceFixture {
    fn new(backend: Arc<dyn LlmBackend>) -> Result<Self> {
        let (temp, session_manager, workspace, rara_dir) = test_runtime_storage();
        let mut agent = Agent::new(
            ToolManager::new(),
            backend,
            Arc::new(MemoryHandle::new(
                &rara_dir.join("memory").to_string_lossy(),
            )),
            session_manager,
            workspace,
        );
        agent.set_session_id("trace-session".to_string());
        let recorder = AgentTraceRecorder::new(rara_dir.join("traces"), agent.session_id.clone())?;
        let events_path = recorder
            .location()
            .ok_or_else(|| anyhow!("trace recorder did not expose a location"))?
            .events_path;
        agent.set_agent_trace_recorder(recorder);
        agent.set_runtime_turn_id(Some("turn-42".to_string()));
        agent.set_memory_facilities_enabled(false);
        Ok(Self {
            _temp: temp,
            agent,
            events_path,
        })
    }
}

struct FailingBackend;

#[async_trait::async_trait]
impl LlmBackend for FailingBackend {
    fn model_label(&self) -> Option<String> {
        Some("trace-error-model".into())
    }

    async fn ask(
        &self,
        _messages: &[Message],
        _tools: &[serde_json::Value],
    ) -> Result<LlmResponse> {
        Err(anyhow!("trace-private-provider-failure"))
    }

    async fn summarize(&self, _messages: &[Message], _instruction: &str) -> Result<String> {
        Ok("summary".into())
    }
}

#[tokio::test]
async fn failed_model_request_records_outcome_without_provider_error_content() -> Result<()> {
    let mut fixture = TraceFixture::new(Arc::new(FailingBackend))?;
    let error = fixture
        .agent
        .query_with_mode(
            "trace-private-failed-prompt".into(),
            AgentOutputMode::Silent,
        )
        .await
        .expect_err("provider request must fail");
    assert!(format!("{error:#}").contains("trace-private-provider-failure"));
    let raw_trace = fs::read_to_string(&fixture.events_path)?;
    assert!(!raw_trace.contains("trace-private-failed-prompt"));
    assert!(!raw_trace.contains("trace-private-provider-failure"));
    let events = raw_trace
        .lines()
        .map(serde_json::from_str::<TraceRecord>)
        .collect::<Result<Vec<_>, _>>()?;
    let models = events
        .iter()
        .filter_map(|record| match &record.event {
            AgentTraceEvent::ModelFinished(model) => Some(model),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        models.len(),
        1,
        "failed requests must retain their completion record"
    );
    assert_eq!(models[0].model, "trace-error-model");
    assert_eq!(models[0].status, TraceModelStatus::Failed);
    assert_eq!(models[0].finish_reason, None);
    assert_eq!(models[0].usage, None);
    assert!(matches!(events.last().map(|record| &record.event),
        Some(AgentTraceEvent::TurnFinished(finished)) if finished.outcome == TraceTurnOutcome::Failed));
    assert!(
        events
            .iter()
            .all(|record| record.turn_id.as_deref() == Some("turn-42"))
    );
    Ok(())
}
