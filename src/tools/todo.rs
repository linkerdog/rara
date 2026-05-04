use async_trait::async_trait;
use rara_tool_macros::tool_spec;
use serde_json::Value;

use crate::todo::normalize_todo_write_input;
use crate::tool::{Tool, ToolError};

pub const TODO_WRITE_TOOL_NAME: &str = "todo_write";

pub struct TodoWriteTool;

#[tool_spec(
    name = "todo_write",
    description = "Create or replace the session todo list for complex multi-step execution. Use this to track mutable execution progress, not to request plan approval.",
    input_schema = {
        "type": "object",
        "properties": {
            "todos": {
                "type": "array",
                "description": "Complete replacement list of todo items for the current session.",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "Optional stable id. If omitted, RARA assigns todo-1, todo-2, and so on."
                        },
                        "content": {
                            "type": "string",
                            "description": "Imperative description of the task."
                        },
                        "status": {
                            "type": "string",
                            "enum": ["pending", "in_progress", "completed", "cancelled"],
                            "description": "Current task status. Keep at most one item in_progress."
                        }
                    },
                    "required": ["content", "status"],
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
                    {"content": "Implement todo_write", "status": "in_progress"},
                    {"content": "Run tests", "status": "pending"}
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
        assert_eq!(
            schema["properties"]["todos"]["items"]["additionalProperties"],
            false
        );
    }
}
