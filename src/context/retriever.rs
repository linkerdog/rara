use std::sync::Arc;

use crate::agent::Message;
use crate::context::assembler::latest_user_request;
use crate::context::{
    RETRIEVED_THREAD_CONTEXT_KIND, RETRIEVED_WORKSPACE_MEMORY_KIND, RetrievedMemoryCandidate,
};
use crate::llm::LlmBackend;
use crate::memory_store::{MemoryRecordSearchHit, MemoryStore};
use crate::session::SessionManager;
use crate::session_context::SessionContextSearchHit;

const WORKSPACE_MEMORY_LIMIT: usize = 6;
const THREAD_CONTEXT_LIMIT: usize = 4;
const MEMORY_DETAIL_MAX_CHARS: usize = 1_200;

pub(crate) struct MemoryRetrievalOrchestrator {
    backend: Arc<dyn LlmBackend>,
    session_manager: Arc<SessionManager>,
    memory_store: Arc<MemoryStore>,
}

impl MemoryRetrievalOrchestrator {
    pub(crate) fn new(
        backend: Arc<dyn LlmBackend>,
        session_manager: Arc<SessionManager>,
        memory_store: Arc<MemoryStore>,
    ) -> Self {
        Self {
            backend,
            session_manager,
            memory_store,
        }
    }

    pub(crate) async fn retrieve_for_history(
        &self,
        history: &[Message],
    ) -> Vec<RetrievedMemoryCandidate> {
        let Some(query) = latest_user_request(history) else {
            return Vec::new();
        };
        self.retrieve_for_query(query.as_str()).await
    }

    async fn retrieve_for_query(&self, query: &str) -> Vec<RetrievedMemoryCandidate> {
        let query = query.trim();
        if query.is_empty() {
            return Vec::new();
        }

        let Ok(query_vector) = self.backend.embed(query).await else {
            return Vec::new();
        };
        let mut candidates = Vec::new();
        candidates.extend(
            self.thread_context_candidates(query, query_vector.clone())
                .await,
        );
        candidates.extend(self.workspace_memory_candidates(query, query_vector).await);
        candidates
    }

    async fn workspace_memory_candidates(
        &self,
        query: &str,
        query_vector: Vec<f32>,
    ) -> Vec<RetrievedMemoryCandidate> {
        let Ok(mut hits) = self
            .memory_store
            .search_with_embedding(query, query_vector, WORKSPACE_MEMORY_LIMIT)
            .await
        else {
            return Vec::new();
        };
        hits.sort_by(|a, b| workspace_rank_score(b).total_cmp(&workspace_rank_score(a)));
        hits.into_iter()
            .enumerate()
            .map(|(idx, hit)| workspace_memory_candidate(idx + 1, hit))
            .collect()
    }

    async fn thread_context_candidates(
        &self,
        query: &str,
        query_vector: Vec<f32>,
    ) -> Vec<RetrievedMemoryCandidate> {
        let session_manager = self.session_manager.clone();
        let query = query.to_string();
        let Ok(Ok(hits)) = tokio::task::spawn_blocking(move || {
            session_manager.search_session_context(&query, &query_vector, THREAD_CONTEXT_LIMIT)
        })
        .await
        else {
            return Vec::new();
        };
        hits.into_iter()
            .enumerate()
            .map(|(idx, hit)| thread_context_candidate(idx + 1, hit))
            .collect()
    }
}

fn workspace_rank_score(hit: &MemoryRecordSearchHit) -> f32 {
    hit.score + hit.record.importance
}

fn workspace_memory_candidate(rank: usize, hit: MemoryRecordSearchHit) -> RetrievedMemoryCandidate {
    let labels = hit
        .record
        .labels
        .iter()
        .map(|label| format!("{label:?}").to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join(",");
    RetrievedMemoryCandidate {
        kind: RETRIEVED_WORKSPACE_MEMORY_KIND.to_string(),
        label: format!("Memory: {}", hit.record.title),
        detail: format!(
            "id={}; scope={:?}; labels={}; importance={:.1}; score={:.3}; content: {}",
            hit.record.id,
            hit.record.scope,
            labels,
            hit.record.importance,
            hit.score,
            truncate_for_memory_context(hit.record.content.as_str(), MEMORY_DETAIL_MAX_CHARS)
        ),
        selection_reason:
            "retrieved as a candidate because LanceDB-backed MemoryStore retrieval matched the current turn query"
                .to_string(),
        rank,
    }
}

fn thread_context_candidate(rank: usize, hit: SessionContextSearchHit) -> RetrievedMemoryCandidate {
    RetrievedMemoryCandidate {
        kind: RETRIEVED_THREAD_CONTEXT_KIND.to_string(),
        label: format!(
            "Session Context {}#{}",
            hit.checkpoint.session_id, hit.checkpoint.turn_index
        ),
        detail: format!(
            "session={}; turn={}; score={:.3}; keyword_score={:.3}; text: {}",
            hit.checkpoint.session_id,
            hit.checkpoint.turn_index,
            hit.score,
            hit.keyword_score,
            truncate_for_memory_context(hit.checkpoint.text.as_str(), MEMORY_DETAIL_MAX_CHARS)
        ),
        selection_reason:
            "retrieved as a candidate because the per-session context shard matched the current turn query"
                .to_string(),
        rank,
    }
}

fn truncate_for_memory_context(value: &str, max_chars: usize) -> String {
    let value = value.trim();
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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use anyhow::Result;
    use async_trait::async_trait;
    use rara_memory::vectordb::VectorDB;
    use serde_json::Value;

    use super::*;
    use crate::llm::{ContentBlock, LlmResponse};
    use crate::memory_store::{MemoryLabel, MemoryScope, MemorySource, NewMemoryRecord};

    #[derive(Default)]
    struct TestBackend {
        embed_calls: AtomicUsize,
    }

    #[async_trait]
    impl LlmBackend for TestBackend {
        async fn ask(&self, _messages: &[Message], _tools: &[Value]) -> Result<LlmResponse> {
            Ok(LlmResponse {
                content: vec![ContentBlock::Text {
                    text: "ok".to_string(),
                }],
                stop_reason: Some("end_turn".to_string()),
                usage: None,
            })
        }

        async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
            self.embed_calls.fetch_add(1, Ordering::SeqCst);
            Ok(vec![1.0; 8])
        }

        async fn summarize(&self, _messages: &[Message], _instruction: &str) -> Result<String> {
            Ok("summary".to_string())
        }
    }

    #[tokio::test]
    async fn retrieves_memory_store_candidates_for_latest_user_request() {
        let temp = tempfile::tempdir().expect("tempdir");
        let backend = Arc::new(TestBackend::default());
        let vdb = Arc::new(VectorDB::new(temp.path().to_str().expect("utf8 path")));
        let store = Arc::new(MemoryStore::new(backend.clone(), vdb.clone()));
        let session_manager = Arc::new(
            SessionManager::new_for_rara_dir(temp.path().join(".rara")).expect("session manager"),
        );
        store
            .insert(NewMemoryRecord {
                title: Some("Reference project path".to_string()),
                content:
                    "Reference project source lives at /Users/example/devel/opensource/reference-project."
                        .to_string(),
                labels: vec![MemoryLabel::Fact],
                importance: 0.9,
                pinned: false,
                source: MemorySource::UserCreated,
                scope: MemoryScope::Workspace,
                session_id: None,
                thread_id: None,
                source_span: None,
            })
            .await
            .expect("insert memory");
        backend.embed_calls.store(0, Ordering::SeqCst);
        let history = vec![Message {
            role: "user".to_string(),
            content: serde_json::json!([
                {"type": "text", "text": "Where is the reference project source path?"}
            ]),
        }];

        let candidates = MemoryRetrievalOrchestrator::new(backend.clone(), session_manager, store)
            .retrieve_for_history(&history)
            .await;

        assert!(candidates.iter().any(|candidate| {
            candidate.kind == RETRIEVED_WORKSPACE_MEMORY_KIND
                && candidate.detail.contains("reference-project")
        }));
        assert_eq!(
            backend.embed_calls.load(Ordering::SeqCst),
            1,
            "orchestrator should embed the latest user query once and reuse it across retrieval sources"
        );
    }

    #[tokio::test]
    async fn retrieves_thread_context_candidates_from_session_shards() {
        let temp = tempfile::tempdir().expect("tempdir");
        let backend = Arc::new(TestBackend::default());
        let vdb = Arc::new(VectorDB::new(temp.path().to_str().expect("utf8 path")));
        let store = Arc::new(MemoryStore::new(backend.clone(), vdb));
        let session_manager = Arc::new(
            SessionManager::new_for_rara_dir(temp.path().join(".rara")).expect("session manager"),
        );
        session_manager
            .save_session_context_checkpoint(
                "session-thread",
                7,
                "approval denial is stored as an errored tool result".to_string(),
                vec![1.0; 8],
            )
            .expect("save session context checkpoint");
        let history = vec![Message {
            role: "user".to_string(),
            content: serde_json::json!([
                {"type": "text", "text": "How did approval denial get recorded?"}
            ]),
        }];

        let candidates = MemoryRetrievalOrchestrator::new(backend, session_manager, store)
            .retrieve_for_history(&history)
            .await;

        assert!(candidates.iter().any(|candidate| {
            candidate.kind == RETRIEVED_THREAD_CONTEXT_KIND
                && candidate.detail.contains("approval denial")
                && candidate.detail.contains("session=session-thread")
        }));
    }
}
