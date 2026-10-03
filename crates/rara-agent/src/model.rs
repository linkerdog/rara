use anyhow::Result;
use rara_core::llm::backend::{LlmBackend, LlmTurnMetadata};
use rara_core::llm::contracts::LlmStreamEvent;
use rara_core::llm::types::{ContentBlock, LlmResponse, Message};
use serde_json::Value;

use crate::ResponseEvidence;

/// One host-prepared request view, shared by dispatch and response observation.
pub struct ModelRequest<'a> {
    pub messages: &'a [Message],
    pub tools: &'a [Value],
    pub metadata: LlmTurnMetadata,
}

/// Provider call identity and the arguments prepared for tool execution.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StreamEvidence {
    pub text_delta: bool,
    pub reasoning_delta: bool,
}

/// Ordered observations emitted while collecting one model response.
#[derive(Clone, Debug)]
pub enum ModelTurnEvent {
    Stream(LlmStreamEvent),
    AssistantText(String),
    ToolUse(ToolCall),
}

#[derive(Debug)]
pub struct ModelTurnOutput {
    pub assistant_message: Option<Message>,
    pub tool_calls: Vec<ToolCall>,
    pub response: ResponseEvidence,
    pub stream: StreamEvidence,
    pub stop_reason: Option<String>,
}

/// Host policy around shared model dispatch and response collection.
///
/// Preserve callback order and provider call identity. Accounting belongs in
/// `observe_response`, which runs even for failed requests. Asynchronous
/// cancellation cleanup remains with the owning `LoopEffects` adapter. Text and
/// tool input policy may fail; no later block or completion callback then runs.
/// These callbacks must not execute the collected tools or commit the transcript.
pub trait ModelTurnPolicy: Send {
    fn event(&mut self, event: ModelTurnEvent);

    fn observe_response(&mut self, _response: &Result<LlmResponse>) {}

    /// Normalize text and emit fallback presentation before host-specific effects.
    /// Overrides preserve suppression of fallback text after streamed text.
    fn prepare_text(&mut self, text: &str, stream: StreamEvidence) -> Result<String> {
        if !stream.text_delta && !text.trim().is_empty() {
            self.event(ModelTurnEvent::AssistantText(text.to_owned()));
        }
        Ok(text.to_owned())
    }

    fn prepare_tool_input(&mut self, call: &ToolCall) -> Result<Value> {
        Ok(call.input.clone())
    }

    fn finish_response(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Execute a model effect without choosing context, native policy, or a runtime.
pub async fn execute_model_turn(
    backend: &dyn LlmBackend,
    request: &ModelRequest<'_>,
    policy: &mut (impl ModelTurnPolicy + ?Sized),
) -> Result<ModelTurnOutput> {
    let mut stream = StreamEvidence::default();
    let response = match request.metadata.ensure_not_cancelled() {
        Ok(()) => {
            backend
                .ask_streaming_with_context(
                    request.messages,
                    request.tools,
                    request.metadata.clone(),
                    &mut |event| {
                        match &event {
                            LlmStreamEvent::TextDelta(_) => stream.text_delta = true,
                            LlmStreamEvent::ReasoningDelta(_) => stream.reasoning_delta = true,
                        }
                        policy.event(ModelTurnEvent::Stream(event));
                    },
                )
                .await
        }
        Err(error) => Err(error),
    };
    policy.observe_response(&response);
    let response = response?;
    let mut evidence = ResponseEvidence {
        had_reasoning_response: stream.reasoning_delta,
        ..Default::default()
    };
    let mut tool_calls = Vec::new();
    let mut content = Vec::new();
    for block in response.content {
        match block {
            ContentBlock::Text { text } => {
                let text = policy.prepare_text(&text, stream)?;
                if !text.trim().is_empty() {
                    evidence.had_text_response = true;
                    content.push(ContentBlock::Text { text });
                }
            }
            ContentBlock::ToolUse { id, name, input } => {
                let mut call = ToolCall { id, name, input };
                // Hooks may change execution arguments, never the provider transcript.
                content.push(ContentBlock::ToolUse {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    input: call.input.clone(),
                });
                call.input = policy.prepare_tool_input(&call)?;
                policy.event(ModelTurnEvent::ToolUse(call.clone()));
                tool_calls.push(call);
            }
            ContentBlock::ProviderMetadata {
                provider,
                key,
                value,
            } => {
                if key == "reasoning_content"
                    && value.as_str().is_some_and(|text| !text.trim().is_empty())
                {
                    evidence.had_reasoning_response = true;
                }
                content.push(ContentBlock::ProviderMetadata {
                    provider,
                    key,
                    value,
                });
            }
        }
    }
    policy.finish_response()?;
    let assistant_message = if evidence.had_text_response || !tool_calls.is_empty() {
        Some(Message {
            role: "assistant".to_owned(),
            content: serde_json::to_value(content)?,
        })
    } else {
        None
    };
    Ok(ModelTurnOutput {
        assistant_message,
        tool_calls,
        response: evidence,
        stream,
        stop_reason: response.stop_reason,
    })
}

#[cfg(test)]
mod tests;
