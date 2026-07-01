use anyhow::Result;
use rara_persistence::thread_data::{
    PersistedCompactState, PersistedInteraction, PersistedPlanStep, PersistedPromptRuntimeState,
    PersistedStructuredRolloutEvent, PersistedThreadLineage, PersistedThreadRecord,
    PersistedTurnEntry, PersistedTurnSummary,
};
use rara_persistence::thread_metadata;
use rara_persistence::thread_rollout_log::RolloutEventRecorder;
use rara_persistence::thread_turn_log;
use rara_state::state_db::StateDb;

use super::{CompactionRecord, write_history_snapshot};
use crate::agent::Message;
use crate::session::PersistedCompactionEvent;
use crate::session_transcript::ThreadTranscriptRecorder;

pub struct ThreadRuntimeState<'a> {
    pub session_id: &'a str,
    pub cwd: &'a str,
    pub branch: &'a str,
    pub provider: &'a str,
    pub model: &'a str,
    pub base_url: Option<&'a str>,
    pub agent_mode: &'a str,
    pub bash_approval: &'a str,
    pub plan_explanation: Option<&'a str>,
    pub prompt_runtime: PersistedPromptRuntimeState,
    pub history_len: usize,
    pub transcript_len: usize,
    pub compact_state: PersistedCompactState,
}

pub struct ThreadRuntimeLineage {
    pub origin_kind: String,
    pub forked_from_thread_id: Option<String>,
}

pub struct ThreadRecorder<'a> {
    state_db: &'a StateDb,
}

impl<'a> ThreadRecorder<'a> {
    pub fn new(state_db: &'a StateDb) -> Self {
        Self { state_db }
    }

    pub fn persist_history_checkpoint(&self, session_id: &str, history: &[Message]) -> Result<()> {
        let rollout_root = self.state_db.rollout_root();
        let recorder = ThreadTranscriptRecorder::main(&rollout_root, session_id);
        recorder.sync_history_checkpoint(history)?;
        recorder.shutdown()?;
        write_history_snapshot(&rollout_root, session_id, history)
    }

    pub fn persist_compaction_event(
        &self,
        session_id: &str,
        event: &PersistedCompactionEvent,
    ) -> Result<()> {
        self.append_rollout_item(
            session_id,
            &PersistedStructuredRolloutEvent::Compaction {
                recorded_at: Some(crate::utils::epoch_seconds()),
                event_index: event.event_index,
                before_tokens: event.before_tokens,
                after_tokens: event.after_tokens,
                boundary_version: event.boundary_version,
                replaced_start: event.replaced_start,
                replaced_end: event.replaced_end,
                metadata_owner: event.metadata_owner.clone(),
                recent_files: event.recent_files.clone(),
                summary: event.summary.clone(),
            },
        )?;
        self.shutdown(session_id)
    }

    pub fn append_rollout_item(
        &self,
        session_id: &str,
        item: &PersistedStructuredRolloutEvent,
    ) -> Result<()> {
        self.rollout_recorder(session_id).append_event(item)
    }

    /// Reserved explicit durability barrier for transcript/event persistence.
    /// The session transcript contract documents both flush and shutdown
    /// boundaries in docs/features/session-transcript.md.
    #[allow(dead_code)]
    pub fn flush(&self, session_id: &str) -> Result<()> {
        self.rollout_recorder(session_id).flush()
    }

    pub fn shutdown(&self, session_id: &str) -> Result<()> {
        self.rollout_recorder(session_id).shutdown()
    }

    fn rollout_recorder(&self, session_id: &str) -> RolloutEventRecorder {
        RolloutEventRecorder::new(&self.state_db.rollout_root(), session_id)
    }

    pub fn persist_runtime_state(&self, state: &ThreadRuntimeState<'_>) -> Result<()> {
        let lineage = self.current_lineage(state.session_id)?;
        self.persist_runtime_state_with_lineage(state, &lineage)
    }

    pub fn persist_runtime_state_with_lineage(
        &self,
        state: &ThreadRuntimeState<'_>,
        lineage: &ThreadRuntimeLineage,
    ) -> Result<()> {
        let now = crate::utils::epoch_seconds();
        let existing_metadata = match thread_metadata::load_thread_record(
            &self.state_db.rollout_root(),
            state.session_id,
        )? {
            Some(record) => Some(record),
            None => self.state_db.load_thread_record(state.session_id)?,
        };
        let created_at = existing_metadata
            .as_ref()
            .map(|record| record.created_at)
            .unwrap_or(now);
        let record = PersistedThreadRecord {
            session_id: state.session_id.to_string(),
            cwd: state.cwd.to_string(),
            branch: state.branch.to_string(),
            provider: state.provider.to_string(),
            model: state.model.to_string(),
            base_url: state.base_url.map(str::to_string),
            agent_mode: state.agent_mode.to_string(),
            bash_approval: state.bash_approval.to_string(),
            created_at,
            lineage: PersistedThreadLineage {
                origin_kind: lineage.origin_kind.clone(),
                forked_from_thread_id: lineage.forked_from_thread_id.clone(),
            },
            plan_explanation: state.plan_explanation.map(str::to_string),
            history_len: state.history_len,
            transcript_len: state.transcript_len,
            updated_at: now,
        };
        thread_metadata::write_thread_record(&self.state_db.rollout_root(), &record)?;
        self.state_db.upsert_session_with_lineage(
            state.session_id,
            state.cwd,
            state.branch,
            state.provider,
            state.model,
            state.base_url,
            state.agent_mode,
            state.bash_approval,
            &PersistedThreadLineage {
                origin_kind: lineage.origin_kind.clone(),
                forked_from_thread_id: lineage.forked_from_thread_id.clone(),
            },
            state.plan_explanation,
            &state.prompt_runtime,
            state.history_len,
            state.transcript_len,
            &state.compact_state,
        )
    }

    fn current_lineage(&self, session_id: &str) -> Result<ThreadRuntimeLineage> {
        let lineage = self
            .state_db
            .load_thread_record(session_id)?
            .map(|record| ThreadRuntimeLineage {
                origin_kind: record.lineage.origin_kind,
                forked_from_thread_id: record.lineage.forked_from_thread_id,
            })
            .unwrap_or(ThreadRuntimeLineage {
                origin_kind: "fresh".to_string(),
                forked_from_thread_id: None,
            });
        Ok(lineage)
    }

    pub fn replace_plan_steps(&self, session_id: &str, steps: &[PersistedPlanStep]) -> Result<()> {
        self.state_db.replace_plan_steps(session_id, steps)
    }

    pub fn replace_interactions(
        &self,
        session_id: &str,
        interactions: &[PersistedInteraction],
    ) -> Result<()> {
        self.state_db.replace_interactions(session_id, interactions)
    }

    pub fn replace_runtime_rollout_events(
        &self,
        session_id: &str,
        items: &[PersistedStructuredRolloutEvent],
    ) -> Result<()> {
        self.append_rollout_item(
            session_id,
            &PersistedStructuredRolloutEvent::runtime_state_from_items(
                items,
                Some(crate::utils::epoch_seconds()),
            ),
        )?;
        self.shutdown(session_id)
    }

    pub fn persist_turn(
        &self,
        session_id: &str,
        ordinal: usize,
        entries: &[PersistedTurnEntry],
    ) -> Result<PersistedTurnSummary> {
        let summary = thread_turn_log::append_turn_record(
            &self.state_db.rollout_root(),
            session_id,
            ordinal,
            entries,
        )?;
        if let Err(err) = self.state_db.persist_turn(session_id, ordinal, entries) {
            eprintln!(
                "Warning: canonical turn log advanced for {session_id}, but StateDb turn index update failed: {err}"
            );
        }
        Ok(summary)
    }
}

pub(super) fn compact_state_from_record(record: &CompactionRecord) -> PersistedCompactState {
    PersistedCompactState {
        compaction_count: record.compaction_count,
        last_compaction_before_tokens: record.before_tokens,
        last_compaction_after_tokens: record.after_tokens,
        last_compaction_recent_file_count: record
            .recent_file_count
            .or(Some(record.recent_files.len()).filter(|value| *value > 0)),
        last_compaction_boundary_version: record.boundary_version,
    }
}
