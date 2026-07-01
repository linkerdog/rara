use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use rara_memory::files::search_memory;
use rara_memory::vectordb::VectorDB;
use rara_tool_macros::tool_spec;
use serde_json::{Value, json};

use crate::tool::{Tool, ToolError};

/// Search project memory using ripgrep text search and LanceDB vector search.
#[derive(Clone)]
pub struct SearchMemoryTool {
    pub rara_home: PathBuf,
    pub vdb: Option<Arc<VectorDB>>,
    /// Optional MemoryQuery hook callback.
    /// Invoked with the search query before the actual search.
    /// Wired at registration time in tooling.rs.
    pub hook_callback: Option<Arc<dyn Fn(&str) + Send + Sync>>,
}

#[tool_spec(
    name = "search_memory",
    description = "Search project memory using ripgrep (text) and LanceDB (vector) search. Returns memory entries matching the query from session files, topics, and MEMORY.md.",
    input_schema = {
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "Search query for text and vector search"
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

        let hits = search_memory(query, &self.rara_home, self.vdb.as_deref())
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        serde_json::to_value(hits)
            .map_err(|e| ToolError::ExecutionFailed(format!("serialization: {e}")))
    }
}
