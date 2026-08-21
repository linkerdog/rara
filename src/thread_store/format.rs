use anyhow::Result;

use super::ThreadSnapshot;
use crate::memory_store::{MemoryPromotionTarget, MemorySource, MemorySourceSpan, NewMemoryRecord};

pub(super) fn format_thread_markdown(thread: &ThreadSnapshot) -> String {
    let title = thread_title(thread);
    let mut lines = vec![
        "---".to_string(),
        format!("title: {}", yaml_scalar(&title)),
        "source: rara".to_string(),
        format!("session_id: {}", yaml_scalar(&thread.metadata.session_id)),
        format!("workspace: {}", yaml_scalar(&thread.metadata.cwd)),
        format!("model: {}", yaml_scalar(&thread.metadata.model)),
        format!("created_at: {}", thread.metadata.created_at),
        format!("updated_at: {}", thread.metadata.updated_at),
        "---".to_string(),
        String::new(),
        format!("# {title}"),
        String::new(),
    ];

    if let Some(summary) = thread
        .compaction
        .summary
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        lines.push("## Summary".to_string());
        lines.push(summary.trim().to_string());
        lines.push(String::new());
    }

    for message in &thread.history {
        lines.push(format!("## {}", role_heading(&message.role)));
        lines.push(render_message_content(&message.content));
        lines.push(String::new());
    }

    lines.join("\n")
}

/// Reserved compatibility formatter for summary-style thread distillation.
/// Activated through `distill_thread_summary` when callers need the legacy
/// one-record path documented in docs/features/memory-records.md.
#[allow(dead_code)]
pub(super) fn thread_summary_memory_record(thread: &ThreadSnapshot) -> Option<NewMemoryRecord> {
    let summary = thread
        .compaction
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| first_user_message(thread));
    let summary = summary?;
    let title = thread_title(thread);
    let mut record = thread_distilled_memory_base(thread).ok()?;
    record.title = Some(title);
    record.content = format!(
        concat!(
            "## Summary\n",
            "{summary}\n\n",
            "## Provenance\n",
            "- session_id: {session_id}\n",
            "- thread_id: {thread_id}\n",
            "- workspace: {workspace}\n",
            "- provider: {provider}\n",
            "- model: {model}\n"
        ),
        summary = summary.as_str(),
        session_id = thread.metadata.session_id.as_str(),
        thread_id = thread.metadata.session_id.as_str(),
        workspace = thread.metadata.cwd.as_str(),
        provider = thread.metadata.provider.as_str(),
        model = thread.metadata.model.as_str(),
    );
    Some(record)
}

pub(super) fn thread_distilled_memory_base(thread: &ThreadSnapshot) -> Result<NewMemoryRecord> {
    let end_turn_index = thread.history.len().saturating_sub(1) as u32;
    NewMemoryRecord::promotion_base(
        MemorySource::ThreadDistill,
        MemoryPromotionTarget::Thread {
            session_id: Some(thread.metadata.session_id.clone()),
            thread_id: thread.metadata.session_id.clone(),
            source_span: Some(MemorySourceSpan {
                start_turn_index: 0,
                end_turn_index,
            }),
        },
    )
}

fn thread_title(thread: &ThreadSnapshot) -> String {
    first_user_message(thread)
        .map(|value| truncate_chars(&collapse_whitespace(&value), 80))
        .unwrap_or_else(|| format!("Thread {}", thread.metadata.session_id))
}

fn first_user_message(thread: &ThreadSnapshot) -> Option<String> {
    thread
        .history
        .iter()
        .find(|message| message.role == "user")
        .map(|message| render_message_content(&message.content))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn role_heading(role: &str) -> &str {
    match role {
        "assistant" => "Assistant",
        "system" => "System",
        "tool" => "Tool",
        "user" => "User",
        _ => "Message",
    }
}

fn render_message_content(content: &serde_json::Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    if let Some(items) = content.as_array() {
        let rendered = items
            .iter()
            .filter_map(render_content_item)
            .collect::<Vec<_>>();
        if !rendered.is_empty() {
            return rendered.join("\n\n");
        }
        if items.iter().any(|item| {
            item.get("type").and_then(serde_json::Value::as_str)
                == Some(crate::model_context::MODEL_CONTEXT_BLOCK_TYPE)
        }) {
            return String::new();
        }
    }
    serde_json::to_string_pretty(content).unwrap_or_else(|_| content.to_string())
}

fn render_content_item(item: &serde_json::Value) -> Option<String> {
    match item.get("type").and_then(serde_json::Value::as_str) {
        Some("text") => item
            .get("text")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        Some("tool_use") => {
            let name = item
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("tool");
            let input = item
                .get("input")
                .map(|value| {
                    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
                })
                .unwrap_or_else(|| "{}".to_string());
            Some(format!("Tool use: {name}\n\n```json\n{input}\n```"))
        }
        Some("tool_result") => item
            .get("content")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        Some(crate::model_context::MODEL_CONTEXT_BLOCK_TYPE) => None,
        _ => Some(serde_json::to_string_pretty(item).unwrap_or_else(|_| item.to_string())),
    }
}

fn yaml_scalar(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::render_message_content;

    #[test]
    fn human_thread_rendering_excludes_model_context() {
        let content = json!([
            {
                "type": "rara_model_context",
                "kind": "retrieved_memory",
                "text": "model-only context must stay out of exports"
            },
            {"type": "text", "text": "human request"}
        ]);

        assert_eq!(render_message_content(&content), "human request");
        assert_eq!(
            render_message_content(&json!([{
                "type": "rara_model_context",
                "kind": "environment",
                "text": "model-only context"
            }])),
            ""
        );
    }
}
