use std::sync::Arc;

use async_trait::async_trait;
use rara_tool_macros::tool_spec;
use serde_json::{Value, json};

use crate::llm::LlmBackend;
use crate::session::SessionManager;
use crate::tool::{Tool, ToolError};
use crate::vectordb::VectorDB;

pub struct RetrieveSessionContextTool {
    pub backend: Arc<dyn LlmBackend>,
    pub vdb: Arc<VectorDB>,
    pub session_manager: Arc<SessionManager>,
}
#[tool_spec(
    name = "retrieve_session_context",
    description = "Recall past context",
    input_schema = { "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] }
)]
#[async_trait]
impl Tool for RetrieveSessionContextTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        let query = input["query"]
            .as_str()
            .ok_or(ToolError::InvalidInput("query".into()))?;
        if query.trim().is_empty() {
            return Ok(json!({ "status": "no_context_found", "matches": [] }));
        }
        let vector = self
            .backend
            .embed(query)
            .await
            .map_err(|err| ToolError::ExecutionFailed(err.to_string()))?;
        let hits = self
            .vdb
            .hybrid_search_with_metadata("conversations", query, vector, 8)
            .await
            .map_err(|err| ToolError::ExecutionFailed(err.to_string()))?;
        if hits.is_empty() {
            return Ok(json!({ "status": "no_context_found", "matches": [] }));
        }
        let matches = hits
            .into_iter()
            .map(|hit| {
                json!({
                    "session_id": hit.metadata.session_id,
                    "turn_index": hit.metadata.turn_index,
                    "text": hit.metadata.text,
                    "score": hit.score,
                    "vector_distance": hit.vector_distance,
                    "fts_score": hit.fts_score,
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "status": "ok",
            "matches": matches,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::MockLlm;
    use crate::vectordb::MemoryMetadata;

    #[tokio::test]
    async fn retrieve_session_context_searches_conversation_index() {
        let temp = tempfile::tempdir().expect("tempdir");
        let vdb = Arc::new(VectorDB::new(temp.path().to_str().expect("utf8 path")));
        vdb.upsert_turn(
            "conversations",
            MemoryMetadata {
                id: None,
                session_id: "session-a".to_string(),
                turn_index: 3,
                text: "The approval denial should be recorded as an errored tool result."
                    .to_string(),
            },
            vec![1.0; 128],
        )
        .await
        .expect("upsert conversation memory");
        let tool = RetrieveSessionContextTool {
            backend: Arc::new(MockLlm),
            vdb,
            session_manager: Arc::new(
                SessionManager::new_for_rara_dir(temp.path().join(".rara"))
                    .expect("session manager"),
            ),
        };

        let result = tool
            .call(json!({ "query": "approval denial errored result" }))
            .await
            .expect("retrieve session context");

        assert_eq!(result["status"], "ok");
        assert_eq!(result["matches"][0]["session_id"], "session-a");
        assert_eq!(result["matches"][0]["turn_index"], 3);
        assert!(
            result["matches"][0]["text"]
                .as_str()
                .is_some_and(|text| text.contains("approval denial"))
        );
    }
}
