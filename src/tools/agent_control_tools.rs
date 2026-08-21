use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rara_tool_macros::tool_spec;
use rara_tools::tool::{Tool, ToolCallContext, ToolError};
use serde_json::{Value, json};

use super::SessionManager;
use super::agent_control::{AgentTreeControl, BackgroundSubAgentStore};
use super::agent_reconnect::{durable_subagent_record, durable_subagent_records};

const MAX_WAIT_TIMEOUT_MS: u64 = 300_000;

fn require_parent_session(context: &ToolCallContext) -> Result<&str, ToolError> {
    context
        .session_id()
        .ok_or_else(|| ToolError::ExecutionFailed("agent control requires a session id".into()))
}

pub struct SubAgentResumeTool {
    pub background_subagents: Arc<BackgroundSubAgentStore>,
    pub session_manager: Arc<SessionManager>,
}

#[tool_spec(
    name = "subagent_resume",
    description = "Resume observing a background sub-agent by agent_id. Returns live in-process status, or reconnects to the current thread's persisted completed sidechain result after a runtime restart, without reading the sidechain transcript into parent context.",
    input_schema = {
        "type": "object",
        "properties": {
            "agent_id": { "type": "string" }
        },
        "required": ["agent_id"]
    }
)]
#[async_trait]
impl Tool for SubAgentResumeTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        self.call_with_context_events(input, ToolCallContext::default(), &mut |_| {})
            .await
    }

    async fn call_with_context_events(
        &self,
        input: Value,
        context: ToolCallContext,
        _report: &mut (dyn FnMut(rara_tools::tool::ToolProgressEvent) + Send),
    ) -> Result<Value, ToolError> {
        let agent_id = input["agent_id"]
            .as_str()
            .ok_or(ToolError::InvalidInput("agent_id".into()))?;
        let parent_session_id = require_parent_session(&context)?;
        match self
            .background_subagents
            .get_for_parent(agent_id, parent_session_id)
        {
            Ok(record) => Ok(record.to_json()),
            Err(err) => {
                durable_subagent_record(&self.session_manager, parent_session_id, agent_id)?
                    .map(|record| record.to_json())
                    .ok_or(err)
            }
        }
    }
}

pub struct SubAgentListTool {
    pub background_subagents: Arc<BackgroundSubAgentStore>,
    pub session_manager: Arc<SessionManager>,
}

#[tool_spec(
    name = "subagent_list",
    description = "List in-process sub-agents plus persisted completed sub-agent edges owned by the current thread. Sidechain transcripts remain on disk and are not loaded into parent context.",
    input_schema = { "type": "object", "properties": {} }
)]
#[async_trait]
impl Tool for SubAgentListTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        self.call_with_context_events(input, ToolCallContext::default(), &mut |_| {})
            .await
    }

    async fn call_with_context_events(
        &self,
        _input: Value,
        context: ToolCallContext,
        _report: &mut (dyn FnMut(rara_tools::tool::ToolProgressEvent) + Send),
    ) -> Result<Value, ToolError> {
        let parent_session_id = require_parent_session(&context)?;
        let mut agents = self
            .background_subagents
            .list_for_parent(parent_session_id)?;
        let mut live_ids = agents
            .iter()
            .map(|record| record.agent_id.clone())
            .collect::<HashSet<_>>();
        match durable_subagent_records(&self.session_manager, parent_session_id) {
            Ok(records) => {
                for record in records {
                    if live_ids.insert(record.agent_id.clone()) {
                        agents.push(record);
                    }
                }
            }
            Err(err) => log::warn!(
                "failed to retrieve durable sub-agent records for parent session {parent_session_id}: {err}"
            ),
        }
        agents.sort_by(|left, right| {
            left.started_at
                .cmp(&right.started_at)
                .then(left.agent_id.cmp(&right.agent_id))
        });
        Ok(json!({
            "subagents": agents.into_iter().map(|record| record.to_json()).collect::<Vec<_>>()
        }))
    }
}

pub struct SubAgentStopTool {
    pub background_subagents: Arc<BackgroundSubAgentStore>,
}

#[tool_spec(
    name = "subagent_stop",
    description = "Request cancellation for a running sub-agent owned by the current session.",
    input_schema = {
        "type": "object",
        "properties": { "agent_id": { "type": "string" } },
        "required": ["agent_id"]
    }
)]
#[async_trait]
impl Tool for SubAgentStopTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        self.call_with_context_events(input, ToolCallContext::default(), &mut |_| {})
            .await
    }

    async fn call_with_context_events(
        &self,
        input: Value,
        context: ToolCallContext,
        _report: &mut (dyn FnMut(rara_tools::tool::ToolProgressEvent) + Send),
    ) -> Result<Value, ToolError> {
        let target = input["agent_id"]
            .as_str()
            .ok_or(ToolError::InvalidInput("agent_id".into()))?;
        let parent_session_id = require_parent_session(&context)?;
        Ok(self
            .background_subagents
            .stop_for_parent(target, parent_session_id)?
            .to_json())
    }
}

pub struct ListAgentsTool {
    pub background_subagents: Arc<BackgroundSubAgentStore>,
}

#[tool_spec(
    name = "list_agents",
    description = "List live and recently completed agents owned by the current runtime session. Returns stable ids, paths, lifecycle state, and bounded result metadata.",
    input_schema = { "type": "object", "properties": {} }
)]
#[async_trait]
impl Tool for ListAgentsTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        self.call_with_context_events(input, ToolCallContext::default(), &mut |_| {})
            .await
    }

    async fn call_with_context_events(
        &self,
        _input: Value,
        context: ToolCallContext,
        _report: &mut (dyn FnMut(rara_tools::tool::ToolProgressEvent) + Send),
    ) -> Result<Value, ToolError> {
        let parent_session_id = require_parent_session(&context)?;
        let mut agents = self
            .background_subagents
            .list_for_parent(parent_session_id)?;
        agents.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(json!({
            "agents": agents.into_iter().map(|record| record.to_json()).collect::<Vec<_>>()
        }))
    }
}

pub struct WaitAgentTool {
    pub background_subagents: Arc<BackgroundSubAgentStore>,
}

#[tool_spec(
    name = "wait_agent",
    description = "Wait for asynchronous agent mailbox activity. This returns delivered messages and does not change child lifecycle state.",
    input_schema = {
        "type": "object",
        "properties": {
            "agent_ids": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Optional agent ids or paths to wait for. Omit to wait for any owned agent."
            },
            "timeout_ms": {
                "type": "integer",
                "minimum": 1,
                "maximum": 300000,
                "default": 10000
            }
        }
    }
)]
#[async_trait]
impl Tool for WaitAgentTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        self.call_with_context_events(input, ToolCallContext::default(), &mut |_| {})
            .await
    }

    async fn call_with_context_events(
        &self,
        input: Value,
        context: ToolCallContext,
        _report: &mut (dyn FnMut(rara_tools::tool::ToolProgressEvent) + Send),
    ) -> Result<Value, ToolError> {
        let parent_session_id = require_parent_session(&context)?;
        let timeout_ms = input
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(10_000);
        if !(1..=MAX_WAIT_TIMEOUT_MS).contains(&timeout_ms) {
            return Err(ToolError::InvalidInput(format!(
                "timeout_ms must be between 1 and {MAX_WAIT_TIMEOUT_MS}"
            )));
        }
        let targets = input
            .get("agent_ids")
            .and_then(Value::as_array)
            .map(|targets| {
                targets
                    .iter()
                    .map(|target| {
                        let target = target.as_str().ok_or_else(|| {
                            ToolError::InvalidInput("agent_ids must contain strings".into())
                        })?;
                        self.background_subagents
                            .get_for_parent(target, parent_session_id)
                            .map(|record| record.agent_id)
                    })
                    .collect::<Result<HashSet<_>, _>>()
            })
            .transpose()?;
        let (messages, timed_out) = self
            .background_subagents
            .wait_for_messages(
                parent_session_id,
                targets.as_ref(),
                Duration::from_millis(timeout_ms),
            )
            .await?;
        Ok(json!({
            "messages": messages.into_iter().map(|message| message.to_json()).collect::<Vec<_>>(),
            "timed_out": timed_out,
        }))
    }
}

pub struct SendAgentMessageTool {
    pub background_subagents: Arc<BackgroundSubAgentStore>,
}

#[tool_spec(
    name = "send_message",
    description = "Send a message to a running agent owned by the current session. Delivery occurs at the child's next model-turn boundary.",
    input_schema = {
        "type": "object",
        "properties": {
            "target": { "type": "string" },
            "message": { "type": "string" }
        },
        "required": ["target", "message"]
    }
)]
#[async_trait]
impl Tool for SendAgentMessageTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        self.call_with_context_events(input, ToolCallContext::default(), &mut |_| {})
            .await
    }

    async fn call_with_context_events(
        &self,
        input: Value,
        context: ToolCallContext,
        _report: &mut (dyn FnMut(rara_tools::tool::ToolProgressEvent) + Send),
    ) -> Result<Value, ToolError> {
        send_message_with_kind(&self.background_subagents, input, context, "message")
    }
}

pub struct FollowupTaskTool {
    pub background_subagents: Arc<BackgroundSubAgentStore>,
}

#[tool_spec(
    name = "followup_task",
    description = "Send a follow-up instruction to a running agent. Completed agents cannot be restarted by this tool.",
    input_schema = {
        "type": "object",
        "properties": {
            "target": { "type": "string" },
            "message": { "type": "string" }
        },
        "required": ["target", "message"]
    }
)]
#[async_trait]
impl Tool for FollowupTaskTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        self.call_with_context_events(input, ToolCallContext::default(), &mut |_| {})
            .await
    }

    async fn call_with_context_events(
        &self,
        input: Value,
        context: ToolCallContext,
        _report: &mut (dyn FnMut(rara_tools::tool::ToolProgressEvent) + Send),
    ) -> Result<Value, ToolError> {
        send_message_with_kind(&self.background_subagents, input, context, "followup")
    }
}

fn send_message_with_kind(
    control: &AgentTreeControl,
    input: Value,
    context: ToolCallContext,
    kind: &str,
) -> Result<Value, ToolError> {
    let parent_session_id = require_parent_session(&context)?;
    let target = input["target"]
        .as_str()
        .ok_or(ToolError::InvalidInput("target".into()))?;
    let message = input["message"]
        .as_str()
        .filter(|message| !message.trim().is_empty())
        .ok_or(ToolError::InvalidInput("message".into()))?;
    let envelope = control.send_to_child(parent_session_id, target, kind, message.to_string())?;
    Ok(json!({ "delivered": true, "message": envelope.to_json() }))
}

pub struct InterruptAgentTool {
    pub background_subagents: Arc<BackgroundSubAgentStore>,
}

#[tool_spec(
    name = "interrupt_agent",
    description = "Request cancellation for a running agent owned by the current session. Capacity is released only after execution exits.",
    input_schema = {
        "type": "object",
        "properties": { "target": { "type": "string" } },
        "required": ["target"]
    }
)]
#[async_trait]
impl Tool for InterruptAgentTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        self.call_with_context_events(input, ToolCallContext::default(), &mut |_| {})
            .await
    }

    async fn call_with_context_events(
        &self,
        input: Value,
        context: ToolCallContext,
        _report: &mut (dyn FnMut(rara_tools::tool::ToolProgressEvent) + Send),
    ) -> Result<Value, ToolError> {
        let parent_session_id = require_parent_session(&context)?;
        let target = input["target"]
            .as_str()
            .ok_or(ToolError::InvalidInput("target".into()))?;
        Ok(self
            .background_subagents
            .stop_for_parent(target, parent_session_id)?
            .to_json())
    }
}
