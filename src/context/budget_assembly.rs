//! Token budget calculation for context assembly.
use serde_json::Value;
use crate::agent::{Message, PlanStepStatus};
use crate::context::assembler::ContextAssembler;
use crate::context::RuntimeInteractionInput;
use crate::llm::{ContextBudget, LlmBackend};

    pub fn budget_for(
        &self,
        backend: &dyn LlmBackend,
        history: &[Message],
        tools: &[Value],
    ) -> Option<ContextBudget> {
        backend.context_budget(history, tools)
    }
}

fn active_turn_budget(
    plan_explanation: Option<&str>,
    plan_steps: &[(PlanStepStatus, String)],
    pending_interactions: &[RuntimeInteractionInput],
    history: &[Message],
) -> usize {
    let plan_budget = plan_explanation
        .map(estimate_text_tokens)
        .unwrap_or_default()
        + plan_steps
            .iter()
            .map(|(_, step)| estimate_text_tokens(step.as_str()))
            .sum::<usize>();
    let interaction_budget = pending_interactions
        .iter()
        .map(|interaction| {
            estimate_text_tokens(interaction.title.as_str())
                + estimate_text_tokens(interaction.summary.as_str())
        })
        .sum::<usize>();
    let latest_request_budget = latest_user_request(history)
        .map(|value| estimate_text_tokens(value.as_str()))
        .unwrap_or_default();
    let tool_budget = latest_tool_results(history)
        .into_iter()
        .map(|(_, detail)| estimate_text_tokens(detail.as_str()))
        .sum::<usize>();

    plan_budget + interaction_budget + latest_request_budget + tool_budget
}

pub(crate) fn latest_user_request(history: &[Message]) -> Option<String> {
    history
        .iter()
        .rev()
        .filter(|message| message.role == "user")
        .find_map(extract_latest_text)
}

pub(crate) fn latest_tool_results(history: &[Message]) -> Vec<(String, String)> {
    history
        .iter()
        .rev()
        .find(|message| message.role == "user" && message.content.as_array().is_some())
        .and_then(|message| message.content.as_array())
        .map(|items| {
            items
                .iter()
                .filter(|&item| item.get("type").and_then(Value::as_str) == Some("tool_result"))
                .map(|item| {
                    let tool_id = item
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .unwrap_or("tool_result")
                        .to_string();
                    let content = item
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    (format!("Tool Result {tool_id}"), content)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn extract_latest_text(message: &Message) -> Option<String> {
    if let Some(text) = message.content.as_str() {
        let trimmed = text.trim();
        return (!trimmed.is_empty()).then(|| trimmed.to_string());
    }
    message.content.as_array().and_then(|items| {
        items
            .iter()
            .rev()
            .find_map(|item| {
                (item.get("type").and_then(Value::as_str) == Some("text"))
                    .then(|| item.get("text").and_then(Value::as_str))
                    .flatten()
            })
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    })
}

pub(crate) fn estimate_text_tokens(text: &str) -> usize {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        0
    } else {
        // Use a conservative text-to-token estimate so local/smaller-context
        // models do not silently overrun the remaining input budget.
        trimmed.len().div_ceil(3)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use anyhow::Result;
    use async_trait::async_trait;
    use rara_config::RaraConfig;
    use serde_json::json;

    use super::*;
    use crate::context::RETRIEVED_WORKSPACE_MEMORY_KIND;
    use crate::llm::{ContentBlock, LlmResponse};

    struct BudgetBackend {
        budget: Option<ContextBudget>,
    }

    #[async_trait]
    impl LlmBackend for BudgetBackend {
        async fn ask(&self, _messages: &[Message], _tools: &[Value]) -> Result<LlmResponse> {
            Ok(LlmResponse {
                content: vec![ContentBlock::Text {
                    text: "ok".to_string(),
                }],
                stop_reason: Some("end_turn".to_string()),
                usage: None,
            })
        }

        async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
            Ok(vec![0.0; 8])
        }

        async fn summarize(&self, _messages: &[Message], _instruction: &str) -> Result<String> {
            Ok("summary".to_string())
        }

        fn context_budget(&self, _messages: &[Message], _tools: &[Value]) -> Option<ContextBudget> {
            self.budget
        }
    }
