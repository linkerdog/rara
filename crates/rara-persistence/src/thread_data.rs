use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedThreadLineage {
    pub origin_kind: String,
    pub forked_from_thread_id: Option<String>,
}

impl Default for PersistedThreadLineage {
    fn default() -> Self {
        Self {
            origin_kind: "fresh".to_string(),
            forked_from_thread_id: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedThreadRecord {
    pub session_id: String,
    pub cwd: String,
    pub branch: String,
    pub provider: String,
    pub model: String,
    pub base_url: Option<String>,
    pub agent_mode: String,
    pub bash_approval: String,
    pub created_at: i64,
    pub lineage: PersistedThreadLineage,
    pub plan_explanation: Option<String>,
    pub history_len: usize,
    pub transcript_len: usize,
    pub updated_at: i64,
}
