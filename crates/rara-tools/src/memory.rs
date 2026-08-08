use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use rara_memory::files::search_memory;
use rara_tool_macros::tool_spec;
use serde_json::{Value, json};

use crate::tool::{Tool, ToolError};

pub type MemoryQueryHook = Arc<dyn Fn(&str) + Send + Sync>;

/// Search project memory using local text files.
#[derive(Clone)]
pub struct SearchMemoryTool {
    pub rara_home: PathBuf,
    /// Optional MemoryQuery hook callback.
    /// Invoked with the search query before the actual search.
    /// Wired at registration time in tooling.rs.
    pub hook_callback: Option<MemoryQueryHook>,
}

#[tool_spec(
    name = "search_memory",
    description = "Search local project memory files with text search. Durable cross-session recall should use the official Mem integration.",
    input_schema = {
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "Search query for local memory text search"
            }
        },
        "required": ["query"]
    }
)]
#[async_trait]
impl Tool for SearchMemoryTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        let query = input["query"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidInput("missing 'query' field".into()))?;

        // MemoryQuery hook: notify hooks before search (non-blocking).
        if let Some(ref cb) = self.hook_callback {
            let cb = cb.clone();
            let q = query.to_owned();
            let _ = tokio::task::spawn_blocking(move || cb(&q)).await;
        }

        let hits = search_memory(query, &self.rara_home)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        serde_json::to_value(hits)
            .map_err(|e| ToolError::ExecutionFailed(format!("serialization: {e}")))
    }
}
