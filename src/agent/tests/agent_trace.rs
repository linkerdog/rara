use std::fs;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use rara_agent_trace::{
    AgentTraceEvent, AgentTraceRecorder, CacheUsage, TraceRecord, TraceTurnOutcome,
};
use rara_memory::memory_handle::MemoryHandle;
use rara_tools::tool::ToolManager;

use super::support::{SequencedBackend, test_runtime_storage};
use crate::agent::{Agent, AgentOutputMode};
use crate::llm::{ContentBlock, LlmResponse, TokenUsage};

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
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
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
    let location = recorder
        .location()
        .ok_or_else(|| anyhow!("trace recorder did not expose a location"))?;
    agent.set_agent_trace_recorder(recorder);
    agent.set_runtime_turn_id(Some("turn-42".to_string()));
    agent.set_memory_facilities_enabled(false);

    agent
        .query_with_mode("trace-secret-prompt".to_string(), AgentOutputMode::Silent)
        .await?;

    let raw_trace = fs::read_to_string(&location.events_path)?;
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
