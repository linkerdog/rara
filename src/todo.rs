use std::collections::HashSet;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TodoState {
    pub version: u32,
    pub items: Vec<TodoItem>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_form: Option<String>,
    pub status: TodoStatus,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

impl TodoStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TodoStatus::Pending => "pending",
            TodoStatus::InProgress => "in_progress",
            TodoStatus::Completed => "completed",
            TodoStatus::Cancelled => "cancelled",
        }
    }
}

impl TodoState {
    pub fn summary(&self) -> TodoSummary {
        let mut summary = TodoSummary {
            total: self.items.len(),
            ..Default::default()
        };
        for item in &self.items {
            match item.status {
                TodoStatus::Pending => summary.pending += 1,
                TodoStatus::InProgress => {
                    summary.in_progress += 1;
                    if summary.active_item.is_none() {
                        summary.active_item = Some(
                            item.active_form
                                .as_deref()
                                .unwrap_or(&item.content)
                                .to_string(),
                        );
                    }
                }
                TodoStatus::Completed => summary.completed += 1,
                TodoStatus::Cancelled => summary.cancelled += 1,
            }
        }
        summary
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TodoSummary {
    pub total: usize,
    pub pending: usize,
    pub in_progress: usize,
    pub completed: usize,
    pub cancelled: usize,
    pub active_item: Option<String>,
}

pub fn normalize_todo_write_input(input: &Value) -> Result<TodoState> {
    let todos = input
        .get("todos")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("todo_write requires a todos array"))?;
    let updated_at = crate::utils::epoch_seconds();
    let mut items = Vec::with_capacity(todos.len());
    let mut in_progress_count = 0usize;
    let mut seen_ids = HashSet::new();

    for (idx, item) in todos.iter().enumerate() {
        let object = item
            .as_object()
            .ok_or_else(|| anyhow!("todo item {} must be an object", idx + 1))?;
        let content = object
            .get("content")
            .or_else(|| object.get("description"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|content| !content.is_empty())
            .ok_or_else(|| anyhow!("todo item {} requires non-empty content", idx + 1))?;
        let status = object
            .get("status")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("todo item {} requires status", idx + 1))?;
        let status = parse_todo_status(status)
            .ok_or_else(|| anyhow!("todo item {} has invalid status '{}'", idx + 1, status))?;
        if status == TodoStatus::InProgress {
            in_progress_count += 1;
        }
        let active_form = object
            .get("activeForm")
            .or_else(|| object.get("active_form"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|active_form| !active_form.is_empty())
            .map(str::to_string);
        let id = object
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("todo-{}", idx + 1));
        if !seen_ids.insert(id.clone()) {
            return Err(anyhow!("todo item {} has duplicate id '{}'", idx + 1, id));
        }
        items.push(TodoItem {
            id,
            content: content.to_string(),
            active_form,
            status,
            updated_at,
        });
    }

    if in_progress_count > 1 {
        return Err(anyhow!("todo_write allows at most one in_progress item"));
    }

    Ok(TodoState {
        version: 1,
        items,
        updated_at,
    })
}

pub fn format_todo_update(state: &TodoState) -> String {
    let summary = state.summary();
    let mut lines = vec![format!(
        "Todo Updated: {} total, {} pending, {} in progress, {} completed, {} cancelled",
        summary.total, summary.pending, summary.in_progress, summary.completed, summary.cancelled
    )];
    if let Some(active) = summary.active_item {
        lines.push(format!("Active: {active}"));
    }
    for item in state.items.iter().take(8) {
        lines.push(format!(
            "{} {}",
            status_marker(&item.status),
            item.content.trim()
        ));
    }
    if state.items.len() > 8 {
        lines.push(format!("... {} more", state.items.len() - 8));
    }
    lines.join("\n")
}

fn status_marker(status: &TodoStatus) -> &'static str {
    match status {
        TodoStatus::Pending => "[ ]",
        TodoStatus::InProgress => "[>]",
        TodoStatus::Completed => "[x]",
        TodoStatus::Cancelled => "[-]",
    }
}

fn parse_todo_status(status: &str) -> Option<TodoStatus> {
    match status {
        "pending" => Some(TodoStatus::Pending),
        "in_progress" => Some(TodoStatus::InProgress),
        "completed" => Some(TodoStatus::Completed),
        "cancelled" => Some(TodoStatus::Cancelled),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn normalizes_todo_write_input() {
        let state = normalize_todo_write_input(&json!({
            "todos": [
                {"content": "Inspect planning runtime", "activeForm": "Inspecting planning runtime", "status": "in_progress"},
                {"id": "verify", "content": "Run focused tests", "activeForm": "Running focused tests", "status": "pending"}
            ]
        }))
        .expect("todo input should normalize");

        assert_eq!(state.version, 1);
        assert_eq!(state.items[0].id, "todo-1");
        assert_eq!(state.items[0].status, TodoStatus::InProgress);
        assert_eq!(state.items[1].id, "verify");
        assert_eq!(
            state.summary().active_item.as_deref(),
            Some("Inspecting planning runtime")
        );
    }

    #[test]
    fn active_form_is_optional_and_snake_case_compatible() {
        let state = normalize_todo_write_input(&json!({
            "todos": [
                {"content": "Inspect planning runtime", "active_form": "Inspecting planning runtime", "status": "in_progress"},
                {"content": "Run focused tests", "status": "pending"}
            ]
        }))
        .expect("todo input should normalize");

        assert_eq!(
            state.items[0].active_form.as_deref(),
            Some("Inspecting planning runtime")
        );
        assert_eq!(state.items[1].active_form, None);
        assert_eq!(
            state.summary().active_item.as_deref(),
            Some("Inspecting planning runtime")
        );
    }

    #[test]
    fn rejects_multiple_in_progress_items() {
        let err = normalize_todo_write_input(&json!({
            "todos": [
                {"content": "First", "status": "in_progress"},
                {"content": "Second", "status": "in_progress"}
            ]
        }))
        .expect_err("multiple active todos should be rejected");

        assert!(err.to_string().contains("at most one in_progress"));
    }

    #[test]
    fn rejects_duplicate_todo_ids() {
        let err = normalize_todo_write_input(&json!({
            "todos": [
                {"content": "Generated id", "status": "pending"},
                {"id": "todo-1", "content": "Explicit collision", "status": "pending"}
            ]
        }))
        .expect_err("duplicate todo ids should be rejected");

        assert!(err.to_string().contains("duplicate id 'todo-1'"));
    }

    #[test]
    fn formats_compact_todo_update() {
        let state = normalize_todo_write_input(&json!({
            "todos": [
                {"content": "Implement todo_write", "status": "completed"},
                {"content": "Wire todo updates", "status": "in_progress"}
            ]
        }))
        .expect("todo input should normalize");

        let rendered = format_todo_update(&state);
        assert!(rendered.contains("Todo Updated: 2 total"));
        assert!(rendered.contains("Active: Wire todo updates"));
        assert!(rendered.contains("[x] Implement todo_write"));
        assert!(rendered.contains("[>] Wire todo updates"));
    }
}
