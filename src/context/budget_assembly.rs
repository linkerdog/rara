// Included from assembler.rs; do not register this file with `mod budget_assembly`.

impl<'a> ContextAssembler<'a> {
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
    let model_context_budget = latest_user_model_context(history)
        .into_iter()
        .map(estimate_text_tokens)
        .sum::<usize>();

    plan_budget
        + interaction_budget
        + latest_request_budget
        + tool_budget
        + model_context_budget
}

fn latest_user_model_context(history: &[Message]) -> Vec<&str> {
    history
        .iter()
        .rev()
        .find(|message| {
            message.role == "user"
                && message.content.as_array().is_some_and(|blocks| {
                    blocks.iter().any(|block| {
                        block.get("type").and_then(Value::as_str) == Some("text")
                    })
                })
        })
        .and_then(|message| message.content.as_array())
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| {
                    crate::model_context::model_context_kind(block)
                        != Some(crate::model_context::ModelContextKind::RetrievedMemory)
                })
                .filter_map(crate::model_context::model_context_text)
                .collect()
        })
        .unwrap_or_default()
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
mod budget_tests {
    use anyhow::Result;
    use async_trait::async_trait;
    use rara_config::RaraConfig;
    use serde_json::json;

    use super::*;
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
        async fn summarize(&self, _messages: &[Message], _instruction: &str) -> Result<String> {
            Ok("summary".to_string())
        }

        fn context_budget(&self, _messages: &[Message], _tools: &[Value]) -> Option<ContextBudget> {
            self.budget
        }
    }

    #[test]
    fn budget_for_passthrough_uses_backend_budget() {
        let workspace = tests::test_workspace();
        let runtime = PromptRuntimeConfig::from_config(&RaraConfig::default());
        let budget = ContextBudget {
            context_window_tokens: 200_000,
            reserved_output_tokens: 4_096,
            compact_threshold_tokens: 190_000,
        };
        let backend = BudgetBackend {
            budget: Some(budget),
        };

        let result = ContextAssembler::new(&workspace, &runtime).budget_for(
            &backend,
            &[Message {
                role: "user".to_string(),
                content: json!([{"type":"text","text":"hello"}]),
            }],
            &[json!({"name":"read_file"})],
        );

        assert_eq!(result, Some(budget));
    }
}
