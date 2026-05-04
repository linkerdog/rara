use std::sync::Arc;

use async_trait::async_trait;
use rara_tool_macros::tool_spec;
use serde_json::{Value, json};

use crate::tool::{Tool, ToolError};
use crate::workspace::WorkspaceMemory;

pub struct UpdateProjectMemoryTool {
    pub workspace: Arc<WorkspaceMemory>,
}
#[tool_spec(
    name = "update_project_memory",
    description = "Update memory.md",
    input_schema = {
        "type": "object",
        "properties": {
            "content": { "type": "string" },
            "append": { "type": "boolean", "default": true }
        },
        "required": ["content"]
    }
)]
#[async_trait]
impl Tool for UpdateProjectMemoryTool {
    async fn call(&self, i: Value) -> Result<Value, ToolError> {
        let content = i["content"]
            .as_str()
            .ok_or(ToolError::InvalidInput("content".into()))?;
        let append = i["append"].as_bool().unwrap_or(true);
        let final_content = if append {
            let mut existing = self.workspace.read_memory_file().unwrap_or_default();
            existing.push_str("\n\n");
            existing.push_str(content);
            existing
        } else {
            content.to_string()
        };
        self.workspace
            .write_memory_file(&final_content)
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        Ok(json!({ "status": "ok" }))
    }
}
