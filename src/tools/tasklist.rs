use std::sync::Arc;

use async_trait::async_trait;
use rara_tool_macros::tool_spec;
use rara_tools::tool::{Tool, ToolError};
use serde::Deserialize;
use serde_json::Value;

use crate::tasklist::{DEFAULT_TASK_LIST_ID, TaskDetails, TaskListStore, task_list_entries};

pub struct TaskListTool {
    pub store: Arc<TaskListStore>,
}

pub struct TaskGetTool {
    pub store: Arc<TaskListStore>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskListInput {
    #[serde(default, alias = "taskListId")]
    task_list_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskGetInput {
    task_id: String,
    #[serde(default, alias = "taskListId")]
    task_list_id: Option<String>,
}

#[tool_spec(
    name = "task_list",
    description = "List shared project tasks with their current status. Use this to find pending, unowned, unblocked work before creating duplicate tasks or after completing assigned work. Results are returned in task ID order and include only summary fields; call task_get with a specific task_id before starting or updating a task.",
    input_schema = {
        "type": "object",
        "properties": {
            "task_list_id": {
                "type": "string",
                "description": "Optional shared task list id. Defaults to the workspace default task list."
            }
        },
        "additionalProperties": false
    }
)]
#[async_trait]
impl Tool for TaskListTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        let input = parse_task_list_input(input)?;
        let tasks = self
            .store
            .list_tasks(input.task_list_id())
            .map_err(|err| ToolError::ExecutionFailed(err.to_string()))?;
        Ok(serde_json::json!({ "tasks": task_list_entries(&tasks) }))
    }
}

#[tool_spec(
    name = "task_get",
    description = "Retrieve full details for a specific shared task. Use this before starting work to verify the task is still current and that blockedBy is empty, and before any future update operation to avoid stale task state.",
    input_schema = {
        "type": "object",
        "properties": {
            "task_id": {
                "type": "string",
                "description": "Task identifier from task_list."
            },
            "task_list_id": {
                "type": "string",
                "description": "Optional shared task list id. Defaults to the workspace default task list."
            }
        },
        "required": ["task_id"],
        "additionalProperties": false
    }
)]
#[async_trait]
impl Tool for TaskGetTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        let input = parse_task_get_input(input)?;
        let task_id = input.task_id.trim();
        if task_id.is_empty() {
            return Err(ToolError::InvalidInput(
                "task_get requires a non-empty task_id".to_string(),
            ));
        }

        let task = self
            .store
            .get_task(input.task_list_id(), task_id)
            .map_err(|err| ToolError::ExecutionFailed(err.to_string()))?
            .map(TaskDetails::from);
        Ok(serde_json::json!({ "task": task }))
    }
}

impl TaskListInput {
    fn task_list_id(&self) -> &str {
        self.task_list_id
            .as_deref()
            .map(str::trim)
            .filter(|task_list_id| !task_list_id.is_empty())
            .unwrap_or(DEFAULT_TASK_LIST_ID)
    }
}

impl TaskGetInput {
    fn task_list_id(&self) -> &str {
        self.task_list_id
            .as_deref()
            .map(str::trim)
            .filter(|task_list_id| !task_list_id.is_empty())
            .unwrap_or(DEFAULT_TASK_LIST_ID)
    }
}

fn parse_task_list_input(input: Value) -> Result<TaskListInput, ToolError> {
    let input = empty_object_for_null(input);
    serde_json::from_value(input).map_err(|err| ToolError::InvalidInput(err.to_string()))
}

fn parse_task_get_input(input: Value) -> Result<TaskGetInput, ToolError> {
    serde_json::from_value(input).map_err(|err| ToolError::InvalidInput(err.to_string()))
}

fn empty_object_for_null(input: Value) -> Value {
    if input.is_null() {
        serde_json::json!({})
    } else {
        input
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;
    use tempfile::tempdir;

    use super::*;
    use crate::tasklist::DEFAULT_TASK_LIST_ID;

    #[tokio::test]
    async fn task_list_returns_summary_entries() {
        let temp = tempdir().expect("tempdir");
        let list_dir = temp.path().join(DEFAULT_TASK_LIST_ID);
        fs::create_dir_all(&list_dir).expect("task list dir");
        write_task(
            &list_dir,
            "1",
            json!({
                "id": "1",
                "subject": "Prepare shared store",
                "status": "completed"
            }),
        );
        write_task(
            &list_dir,
            "2",
            json!({
                "id": "2",
                "subject": "Add read tools",
                "status": "pending",
                "owner": "agent-a",
                "blockedBy": ["1", "3"]
            }),
        );

        let tool = TaskListTool {
            store: Arc::new(TaskListStore::new(temp.path())),
        };
        let output = tool.call(json!({})).await.expect("task_list should work");

        assert_eq!(output["tasks"][0]["id"], "1");
        assert_eq!(output["tasks"][1]["owner"], "agent-a");
        assert_eq!(output["tasks"][1]["blockedBy"], json!(["3"]));
    }

    #[tokio::test]
    async fn task_get_returns_full_details_or_null() {
        let temp = tempdir().expect("tempdir");
        let list_dir = temp.path().join(DEFAULT_TASK_LIST_ID);
        fs::create_dir_all(&list_dir).expect("task list dir");
        write_task(
            &list_dir,
            "2",
            json!({
                "id": "2",
                "subject": "Add read tools",
                "description": "Expose summary and detail reads.",
                "status": "pending",
                "blocks": ["3"],
                "blockedBy": ["1"]
            }),
        );

        let tool = TaskGetTool {
            store: Arc::new(TaskListStore::new(temp.path())),
        };
        let output = tool
            .call(json!({ "task_id": "2" }))
            .await
            .expect("task_get should work");
        let missing = tool
            .call(json!({ "task_id": "missing" }))
            .await
            .expect("missing task should be valid");

        assert_eq!(
            output["task"]["description"],
            "Expose summary and detail reads."
        );
        assert_eq!(output["task"]["blockedBy"], json!(["1"]));
        assert!(missing["task"].is_null());
    }

    #[test]
    fn task_tool_schemas_are_strict() {
        assert_eq!(
            TaskListTool::input_schema(&tool_list()).get("additionalProperties"),
            Some(&json!(false))
        );
        assert_eq!(
            TaskGetTool::input_schema(&tool_get()).get("additionalProperties"),
            Some(&json!(false))
        );
    }

    #[tokio::test]
    async fn task_get_rejects_empty_task_id() {
        let tool = tool_get();
        let err = tool
            .call(json!({ "task_id": " " }))
            .await
            .expect_err("empty task id should fail");

        assert!(err.to_string().contains("non-empty task_id"));
    }

    fn tool_list() -> TaskListTool {
        let temp = tempdir().expect("tempdir");
        TaskListTool {
            store: Arc::new(TaskListStore::new(temp.path())),
        }
    }

    fn tool_get() -> TaskGetTool {
        let temp = tempdir().expect("tempdir");
        TaskGetTool {
            store: Arc::new(TaskListStore::new(temp.path())),
        }
    }

    fn write_task(dir: &std::path::Path, id: &str, task: serde_json::Value) {
        fs::write(
            dir.join(format!("{id}.json")),
            serde_json::to_vec_pretty(&task).expect("serialize task"),
        )
        .expect("write task");
    }
}
