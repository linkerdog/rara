use serde_json::{Value, json};

use crate::agent::Message;
use crate::model_context::{MODEL_CONTEXT_BLOCK_TYPE, model_context_text};

pub(super) fn scenario_token_cap(messages: &[Message], tools: &[Value]) -> usize {
    if !tools.is_empty() {
        return 128;
    }

    let last_user_text = messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .map(|message| render_human_text(&message.content))
        .unwrap_or_default();
    let normalized = last_user_text.to_ascii_lowercase();
    let trimmed = last_user_text.trim();

    if trimmed.chars().count() <= 12 {
        96
    } else if [
        "summarize",
        "summary",
        "rewrite",
        "explain in detail",
        "detailed",
        "long-form",
    ]
    .iter()
    .any(|keyword| normalized.contains(keyword))
    {
        320
    } else {
        192
    }
}

fn render_human_text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    content
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter(|item| item.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

pub(super) fn build_agent_prompt(messages: &[Message], tools: &[Value]) -> String {
    let tool_schemas = if tools.is_empty() {
        "[]".to_string()
    } else {
        serde_json::to_string_pretty(tools).unwrap_or_else(|_| "[]".to_string())
    };

    format!(
        "You are the local model backend for RARA.\n\
         You are participating in an agent loop with tools.\n\
         Return exactly one JSON object and nothing else.\n\n\
         Valid reply shapes:\n\
         {{\"kind\":\"final\",\"text\":\"final answer for the user\"}}\n\
         {{\"kind\":\"tool\",\"text\":\"optional short reasoning\",\"calls\":[{{\"name\":\"tool_name\",\"input\":{{}}}}]}}\n\n\
         Rules:\n\
         - Use kind=\"tool\" only when a tool is required.\n\
         - Tool names must match the provided schema exactly.\n\
         - Tool inputs must be valid JSON objects.\n\
         - Do not use markdown fences.\n\
         - If the task is completed, use kind=\"final\".\n\n\
         Available tools:\n{tool_schemas}\n\n\
         Conversation:\n{}",
        render_messages(messages)
    )
}

pub(super) fn render_messages(messages: &[Message]) -> String {
    let mut out = String::new();
    for message in messages {
        out.push_str(&format!(
            "{}:\n{}\n\n",
            message.role.to_uppercase(),
            render_content(&message.content)
        ));
    }
    out
}

pub(super) fn render_content(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    if let Some(items) = content.as_array() {
        let mut rendered = Vec::new();
        for item in items {
            match item.get("type").and_then(Value::as_str) {
                Some("text") => rendered.push(
                    item.get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                ),
                Some(MODEL_CONTEXT_BLOCK_TYPE) => {
                    rendered.push(model_context_text(item).unwrap_or_default().to_string())
                }
                Some("tool_result") => rendered.push(format!(
                    "tool_result(id={}): {}",
                    item.get("tool_use_id")
                        .and_then(Value::as_str)
                        .unwrap_or(""),
                    item.get("content").and_then(Value::as_str).unwrap_or("")
                )),
                Some("tool_use") => rendered.push(format!(
                    "tool_use(name={}, id={}, input={})",
                    item.get("name").and_then(Value::as_str).unwrap_or(""),
                    item.get("id").and_then(Value::as_str).unwrap_or(""),
                    item.get("input").cloned().unwrap_or_else(|| json!({}))
                )),
                _ => rendered.push(item.to_string()),
            }
        }
        return rendered.join("\n");
    }
    content.to_string()
}
