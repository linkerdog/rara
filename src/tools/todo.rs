use async_trait::async_trait;
use rara_tool_macros::tool_spec;
use rara_tools::tool::{Tool, ToolError};
use serde_json::Value;

use crate::todo::normalize_todo_write_input;

pub const TODO_WRITE_TOOL_NAME: &str = "todo_write";

pub struct TodoWriteTool;

#[tool_spec(
    name = "todo_write",
    description = "Create or replace the session todo list for complex multi-step execution. Use this proactively once work has multiple concrete steps or verification work worth tracking; do not use it for trivial one-step tasks or to request plan approval. Re-send the full working set whenever statuses, order, blockers, or validation steps change. Keep at most one item in_progress, update statuses promptly, and provide both content (imperative) and activeForm (present continuous label shown while in progress) for each item. Prefer concrete execution items such as reproducing a bug, running a focused test, or final verification. Do not mark an item completed until the underlying implementation or validation is actually done.",
    input_schema = {
        "type": "object",
        "properties": {
            "todos": {
                "type": "array",
                "description": "Complete replacement list of todo items for the current session. Re-send the entire working set whenever items, order, statuses, blockers, or verification work changes.",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "Optional stable id. If omitted, RARA assigns todo-1, todo-2, and so on."
                        },
                        "content": {
                            "type": "string",
                            "description": "Short imperative task description. Prefer concrete work items such as 'Reproduce failing behavior' or 'Run focused regression test'."
                        },
                        "activeForm": {
                            "type": "string",
                            "description": "Present continuous label shown while this item is in_progress, such as 'Running focused tests'."
                        },
                        "status": {
                            "type": "string",
                            "enum": ["pending", "in_progress", "completed", "cancelled"],
                            "description": "Current task status. Keep at most one item in_progress, and do not mark completed until the relevant implementation or verification work is actually done."
                        }
                    },
                    "required": ["content", "status", "activeForm"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["todos"],
        "additionalProperties": false
    }
)]
#[async_trait]
impl Tool for TodoWriteTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        let state = normalize_todo_write_input(&input)
            .map_err(|err| ToolError::InvalidInput(err.to_string()))?;
        serde_json::to_value(state).map_err(|err| ToolError::ExecutionFailed(err.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::todo::TodoStatus;

    #[tokio::test]
    async fn todo_write_returns_normalized_state() {
        let tool = TodoWriteTool;
        let result = tool
            .call(json!({
                "todos": [
                    {"content": "Implement todo_write", "activeForm": "Implementing todo_write", "status": "in_progress"},
                    {"content": "Run tests", "activeForm": "Running tests", "status": "pending"}
                ]
            }))
            .await
            .expect("todo_write should normalize state");
        let state: crate::todo::TodoState =
            serde_json::from_value(result).expect("result should be todo state");

        assert_eq!(state.items.len(), 2);
        assert_eq!(state.items[0].id, "todo-1");
        assert_eq!(state.items[0].status, TodoStatus::InProgress);
    }

    #[test]
    fn todo_write_schema_is_strict_compatible() {
        let schema = TodoWriteTool.input_schema();

        assert_eq!(schema["additionalProperties"], false);
        assert!(
            schema["properties"]["todos"]["items"]["required"]
                .as_array()
                .expect("required fields")
                .iter()
                .any(|field| field == "activeForm")
        );
        assert_eq!(
            schema["properties"]["todos"]["items"]["additionalProperties"],
            false
        );
    }

    #[test]
    fn todo_write_description_guides_execution_and_verification() {
        let tool = TodoWriteTool;
        let description = tool.description();
        assert!(description.contains("multiple concrete steps"));
        assert!(description.contains("verification work worth tracking"));
        assert!(description.contains("Re-send the full working set"));
        assert!(description.contains("activeForm"));
        assert!(description.contains("present continuous"));
        assert!(description.contains("running a focused test"));
        assert!(description.contains("underlying implementation or validation"));

        let schema = tool.input_schema().to_string();
        assert!(schema.contains("entire working set"));
        assert!(schema.contains("Reproduce failing behavior"));
        assert!(schema.contains("Running focused tests"));
        assert!(schema.contains("relevant implementation or verification work"));
    }
}
