use rara_persistence::thread_data::{
    PersistedCompactState, PersistedInteraction, PersistedPlanStep, PersistedRecentThreadRecord,
    PersistedThreadRecord, PersistedTurnEntry, PersistedTurnSummary,
};

use crate::agent::Message;
use crate::session::PersistedCompactionEvent;

#[derive(Debug, Clone, Default)]
pub struct CompactionRecord {
    pub compaction_count: usize,
    pub before_tokens: Option<usize>,
    pub after_tokens: Option<usize>,
    pub recent_file_count: Option<usize>,
    pub boundary_version: Option<u32>,
    pub replaced_start: Option<usize>,
    pub replaced_end: Option<usize>,
    pub metadata_owner: Option<String>,
    pub recent_files: Vec<String>,
    pub summary: Option<String>,
}

impl From<PersistedCompactState> for CompactionRecord {
    fn from(value: PersistedCompactState) -> Self {
        Self {
            compaction_count: value.compaction_count,
            before_tokens: value.last_compaction_before_tokens,
            after_tokens: value.last_compaction_after_tokens,
            recent_file_count: value.last_compaction_recent_file_count,
            boundary_version: value.last_compaction_boundary_version,
            replaced_start: None,
            replaced_end: None,
            metadata_owner: None,
            recent_files: Vec::new(),
            summary: None,
        }
    }
}

impl From<PersistedCompactionEvent> for CompactionRecord {
    fn from(value: PersistedCompactionEvent) -> Self {
        Self {
            compaction_count: value.event_index,
            before_tokens: Some(value.before_tokens),
            after_tokens: Some(value.after_tokens),
            recent_file_count: Some(value.recent_files.len()),
            boundary_version: Some(value.boundary_version),
            replaced_start: value.replaced_start,
            replaced_end: value.replaced_end,
            metadata_owner: value.metadata_owner,
            recent_files: value.recent_files,
            summary: Some(value.summary),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ThreadMetadata {
    pub session_id: String,
    pub cwd: String,
    pub branch: String,
    pub provider: String,
    pub model: String,
    pub base_url: Option<String>,
    pub agent_mode: String,
    pub bash_approval: String,
    pub created_at: i64,
    pub origin_kind: String,
    pub forked_from_thread_id: Option<String>,
    pub history_len: usize,
    pub transcript_len: usize,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct ThreadSummary {
    pub metadata: ThreadMetadata,
    pub preview: String,
    pub compaction: CompactionRecord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadMetadataSource {
    StructuredMetadata,
    StateDb,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadHistorySource {
    CanonicalHistory,
    HistorySnapshotBackfilled,
    LegacyHistoryBackfilled,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadNonTurnRolloutSource {
    StructuredEventsLog,
    LegacyBackfilled,
    StateDbFallback,
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadMaterializationProvenance {
    pub metadata_source: ThreadMetadataSource,
    pub history_source: ThreadHistorySource,
    pub non_turn_rollout_source: ThreadNonTurnRolloutSource,
}

impl ThreadMaterializationProvenance {
    /// Human-readable description of where each piece of the thread snapshot
    /// was sourced from, making the legacy-fallback hierarchy explicit.
    pub fn describe(&self) -> String {
        let metadata = match self.metadata_source {
            ThreadMetadataSource::StructuredMetadata => "structured thread metadata",
            ThreadMetadataSource::StateDb => "StateDb fallback",
        };
        let history = match self.history_source {
            ThreadHistorySource::CanonicalHistory => "canonical transcript.jsonl",
            ThreadHistorySource::HistorySnapshotBackfilled => "history.json snapshot (backfilled)",
            ThreadHistorySource::LegacyHistoryBackfilled => "legacy session JSON (backfilled)",
            ThreadHistorySource::Missing => "missing",
        };
        let rollout = match self.non_turn_rollout_source {
            ThreadNonTurnRolloutSource::StructuredEventsLog => "structured events log (canonical)",
            ThreadNonTurnRolloutSource::LegacyBackfilled => "legacy rollout (backfilled)",
            ThreadNonTurnRolloutSource::StateDbFallback => "StateDb fallback",
            ThreadNonTurnRolloutSource::Empty => "empty",
        };
        format!("metadata={metadata} history={history} non-turn-rollout={rollout}")
    }
}

impl From<PersistedThreadRecord> for ThreadMetadata {
    fn from(value: PersistedThreadRecord) -> Self {
        Self {
            session_id: value.session_id,
            cwd: value.cwd,
            branch: value.branch,
            provider: value.provider,
            model: value.model,
            base_url: value.base_url,
            agent_mode: value.agent_mode,
            bash_approval: value.bash_approval,
            created_at: value.created_at,
            origin_kind: value.lineage.origin_kind,
            forked_from_thread_id: value.lineage.forked_from_thread_id,
            history_len: value.history_len,
            transcript_len: value.transcript_len,
            updated_at: value.updated_at,
        }
    }
}

impl ThreadMetadata {
    /// Returns true when this thread was forked from another thread.
    pub fn is_fork(&self) -> bool {
        self.origin_kind == "fork" && self.forked_from_thread_id.is_some()
    }

    /// Returns the origin kind and optional source thread id, making the
    /// lineage explicit for callers that need to trace thread ancestry.
    pub fn lineage(&self) -> (&str, Option<&str>) {
        (
            self.origin_kind.as_str(),
            self.forked_from_thread_id.as_deref(),
        )
    }
}

impl From<PersistedRecentThreadRecord> for ThreadSummary {
    fn from(value: PersistedRecentThreadRecord) -> Self {
        Self {
            metadata: ThreadMetadata {
                session_id: value.session_id,
                cwd: value.cwd,
                branch: value.branch,
                provider: value.provider,
                model: value.model,
                base_url: value.base_url,
                agent_mode: value.agent_mode,
                bash_approval: value.bash_approval,
                created_at: value.created_at,
                origin_kind: value.lineage.origin_kind,
                forked_from_thread_id: value.lineage.forked_from_thread_id,
                history_len: value.history_len,
                transcript_len: value.transcript_len,
                updated_at: value.updated_at,
            },
            preview: value.preview,
            compaction: CompactionRecord {
                compaction_count: value.compaction_count,
                before_tokens: value.last_compaction_before_tokens,
                after_tokens: value.last_compaction_after_tokens,
                recent_file_count: value.last_compaction_recent_file_count,
                boundary_version: value.last_compaction_boundary_version,
                replaced_start: None,
                replaced_end: None,
                metadata_owner: None,
                recent_files: Vec::new(),
                summary: None,
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct RolloutTurnItem {
    pub summary: PersistedTurnSummary,
    pub entries: Vec<PersistedTurnEntry>,
}

#[derive(Debug, Clone)]
pub enum RolloutItem {
    Compaction(CompactionRecord),
    PlanState {
        explanation: Option<String>,
        steps: Vec<PersistedPlanStep>,
    },
    Interaction(PersistedInteraction),
    SpawnAgent {
        event_id: String,
        agent_id: String,
        name: Option<String>,
        child_session_id: String,
        status: String,
        summary: Option<String>,
    },
    Turn(RolloutTurnItem),
}

#[derive(Debug, Clone)]
pub struct ThreadSnapshot {
    pub metadata: ThreadMetadata,
    pub provenance: ThreadMaterializationProvenance,
    pub history: Vec<Message>,
    pub compaction: CompactionRecord,
    pub plan_explanation: Option<String>,
    pub plan_steps: Vec<PersistedPlanStep>,
    pub interactions: Vec<PersistedInteraction>,
    pub rollout_items: Vec<RolloutItem>,
}

impl ThreadSnapshot {
    /// Human-readable provenance description showing the source hierarchy.
    pub fn provenance_description(&self) -> String {
        self.provenance.describe()
    }

    /// Returns true when this snapshot was forked from another thread.
    pub fn is_fork(&self) -> bool {
        self.metadata.is_fork()
    }

    pub fn lineage(&self) -> (&str, Option<&str>) {
        self.metadata.lineage()
    }
}
