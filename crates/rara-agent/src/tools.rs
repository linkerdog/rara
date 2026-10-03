use anyhow::Result;
use async_trait::async_trait;
use rara_core::llm::types::Message;
use rara_core::tool::{Tool, ToolCallContext, ToolError, ToolProgressEvent};
use serde_json::{Value, json};

use crate::{ToolBatchOutcome, ToolCall};

/// Model-visible output after host result policy has completed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolReply {
    pub content: String,
    pub is_error: bool,
}

impl ToolReply {
    pub fn success(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
        }
    }

    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
        }
    }

    pub fn into_message(self, call_id: &str) -> Message {
        let mut block = json!({
            "type": "tool_result", "tool_use_id": call_id, "content": self.content,
        });
        if self.is_error {
            block["is_error"] = json!(true);
        }
        Message {
            role: "user".into(),
            content: json!([block]),
        }
    }
}

/// Explicit host admission; the executor never grants permission itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolAdmission {
    Invoke,
    Reply(ToolReply),
    AwaitingApproval,
    /// The host has diagnosed an omission and intentionally supplied no reply.
    Omit,
}

#[derive(Debug)]
pub struct ToolBatchOutput {
    pub messages: Vec<Message>,
    pub outcome: ToolBatchOutcome,
}

/// Session-owned policy and effects for serial tool execution.
///
/// Admission must enforce host permissions before invocation. Complete result
/// processing before returning a reply; errors stop further admission. Tool
/// errors are given to result policy, which decides whether to return an error
/// reply or fail the batch. Implementors preserve cooperative cleanup barriers.
#[async_trait]
pub trait ToolBatchEffects: Send {
    async fn begin_batch(&mut self, _calls: &[ToolCall]) -> Result<()> {
        Ok(())
    }

    async fn prepare_call(&mut self, call: &ToolCall) -> Result<ToolAdmission>;

    async fn invoke_call(&mut self, call: &ToolCall) -> Result<Value, ToolError>;

    async fn complete_call(
        &mut self,
        call: &ToolCall,
        result: Result<Value, ToolError>,
    ) -> Result<ToolReply>;
}

/// Await every admitted call and its result policy before admitting another.
pub async fn execute_tool_batch(
    calls: Vec<ToolCall>,
    effects: &mut (impl ToolBatchEffects + ?Sized),
) -> Result<ToolBatchOutput> {
    effects.begin_batch(&calls).await?;
    let mut messages = Vec::new();
    for call in calls {
        let reply = match effects.prepare_call(&call).await? {
            ToolAdmission::Invoke => {
                let result = effects.invoke_call(&call).await;
                effects.complete_call(&call, result).await?
            }
            ToolAdmission::Reply(reply) => reply,
            ToolAdmission::AwaitingApproval => {
                return Ok(ToolBatchOutput {
                    messages,
                    outcome: ToolBatchOutcome::AwaitingApproval,
                });
            }
            ToolAdmission::Omit => continue,
        };
        messages.push(reply.into_message(&call.id));
    }
    Ok(ToolBatchOutput {
        messages,
        outcome: ToolBatchOutcome::ResultsAvailable,
    })
}

/// Tool progress bound to the same provider call as its invocation and result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCallProgress {
    pub call_id: String,
    pub name: String,
    pub event: ToolProgressEvent,
}

/// Invoke a host-selected tool with trusted context and provider call identity.
///
/// Cancellation is forwarded, not synthesized into a premature completion.
pub async fn execute_tool_call(
    tool: &dyn Tool,
    call: &ToolCall,
    context: ToolCallContext,
    report: &mut (dyn FnMut(ToolCallProgress) + Send),
) -> Result<Value, ToolError> {
    tool.call_with_context_events(
        call.input.clone(),
        context.with_call_id(&call.id),
        &mut |event| {
            report(ToolCallProgress {
                call_id: call.id.clone(),
                name: call.name.clone(),
                event,
            })
        },
    )
    .await
}

#[cfg(test)]
mod tests;
