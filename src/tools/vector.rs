use std::sync::Arc;

use async_trait::async_trait;
use rara_tool_macros::tool_spec;
use serde_json::{Value, json};

use crate::llm::LlmBackend;
use crate::memory_store::{MemoryStore, NewMemoryRecord};
use crate::tool::{Tool, ToolError};
use crate::vectordb::VectorDB;

pub struct RememberExperienceTool {
    pub backend: Arc<dyn LlmBackend>,
    pub vdb: Arc<VectorDB>,
    pub db_uri: String,
}
#[tool_spec(
    name = "remember_experience",
    description = "Save insight",
    input_schema = { "type": "object", "properties": { "experience": { "type": "string" } }, "required": ["experience"] }
)]
#[async_trait]
impl Tool for RememberExperienceTool {
    async fn call(&self, i: Value) -> Result<Value, ToolError> {
        let text = i["experience"]
            .as_str()
            .ok_or(ToolError::InvalidInput("experience".into()))?;
        let store = MemoryStore::new(self.backend.clone(), self.vdb.clone());
        let record = store
            .insert(NewMemoryRecord::experience(text))
            .await
            .map_err(|err| ToolError::ExecutionFailed(err.to_string()))?;
        Ok(
            json!({ "status": "ok", "id": record.id, "saved": record.content, "store": self.db_uri }),
        )
    }
}

pub struct RetrieveExperienceTool {
    pub backend: Arc<dyn LlmBackend>,
    pub vdb: Arc<VectorDB>,
    pub db_uri: String,
}
#[tool_spec(
    name = "retrieve_experience",
    description = "Retrieve past insights",
    input_schema = { "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] }
)]
#[async_trait]
impl Tool for RetrieveExperienceTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        let query = input["query"]
            .as_str()
            .ok_or(ToolError::InvalidInput("query".into()))?;
        let store = MemoryStore::new(self.backend.clone(), self.vdb.clone());
        let hits = store
            .search(query, 8)
            .await
            .map_err(|err| ToolError::ExecutionFailed(err.to_string()))?;
        let relevant_experiences = hits
            .iter()
            .map(|hit| hit.record.content.clone())
            .collect::<Vec<_>>();
        let diagnostics = hits
            .iter()
            .map(|hit| {
                json!({
                    "id": &hit.record.id,
                    "title": &hit.record.title,
                    "labels": &hit.record.labels,
                    "importance": hit.record.importance,
                    "source": &hit.record.source,
                    "scope": &hit.record.scope,
                    "score": hit.score,
                    "vector_distance": hit.vector_distance,
                    "fts_score": hit.fts_score,
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "relevant_experiences": relevant_experiences,
            "diagnostics": diagnostics,
            "store": self.db_uri,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::MockLlm;

    #[tokio::test]
    async fn remember_and_retrieve_experience_use_lancedb_hybrid_index() {
        let temp = tempfile::tempdir().expect("tempdir");
        let vdb = Arc::new(VectorDB::new(temp.path().to_str().expect("utf8 path")));
        let backend = Arc::new(MockLlm);
        let remember = RememberExperienceTool {
            backend: backend.clone(),
            vdb: vdb.clone(),
            db_uri: vdb.uri().to_string(),
        };
        remember
            .call(json!({ "experience": "DeepSeek DSML requires a structured parser." }))
            .await
            .expect("remember experience");

        let retrieve = RetrieveExperienceTool {
            backend,
            vdb,
            db_uri: temp.path().display().to_string(),
        };
        let result = retrieve
            .call(json!({ "query": "DSML parser" }))
            .await
            .expect("retrieve experience");
        let experiences = result["relevant_experiences"]
            .as_array()
            .expect("experience array");
        assert_eq!(experiences.len(), 1);
        assert_eq!(
            experiences[0].as_str(),
            Some("DeepSeek DSML requires a structured parser.")
        );
        assert!(
            result["diagnostics"]
                .as_array()
                .is_some_and(|items| !items.is_empty())
        );
    }
}
