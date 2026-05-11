// Items reserved for thread store persistence.
#![allow(dead_code)]
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use rara_persistence::atomic_file;
use rara_persistence::thread_data::{
    PersistedCompactState, PersistedInteraction, PersistedLegacyRolloutMigration,
    PersistedLegacyRolloutSource, PersistedPlanStep, PersistedPromptRuntimeState,
    PersistedRecentThreadRecord, PersistedRuntimeRolloutItem, PersistedStructuredRolloutEvent,
    PersistedThreadLineage, PersistedThreadRecord, PersistedTurnEntry, PersistedTurnSummary,
};
use rara_persistence::thread_metadata;
use rara_persistence::thread_rollout_log::{self, RolloutEventRecorder};
use rara_persistence::thread_turn_log;
use rara_state::state_db::StateDb;
use uuid::Uuid;

use crate::agent::Message;
use crate::memory_distiller::{
    MemoryDistiller, dedupe_memory_drafts, new_memory_record_from_draft,
};
use crate::memory_store::{
    MemoryPromotionTarget, MemoryRecord, MemorySource, MemorySourceSpan, MemoryStore,
    NewMemoryRecord,
};
use crate::session::{
    PersistedCompactionEvent, PersistedCompactionEventsMigration, PersistedCompactionEventsSource,
    PersistedThreadHistoryMigration, PersistedThreadHistorySource, SessionManager,
};
use crate::session_transcript::{self, ThreadTranscriptRecorder};

#[cfg(test)]
mod tests;

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

#[derive(Debug, Clone)]
struct ThreadMaterializedState {
    metadata: ThreadMetadata,
    provenance: ThreadMaterializationProvenance,
    history: Vec<Message>,
    compaction: CompactionRecord,
    plan_explanation: Option<String>,
    plan_steps: Vec<PersistedPlanStep>,
    interactions: Vec<PersistedInteraction>,
    rollout_items: Vec<RolloutItem>,
}

#[derive(Debug, Clone, Default)]
struct LegacyNonTurnRolloutMigration {
    structured_events: Vec<PersistedStructuredRolloutEvent>,
    runtime_rollout: Vec<PersistedRuntimeRolloutItem>,
    compaction_events: Vec<PersistedCompactionEvent>,
    rollout_source: PersistedLegacyRolloutSource,
    compaction_source: PersistedCompactionEventsSource,
}

pub struct ThreadStore<'a> {
    rollout_root: PathBuf,
    legacy_session_root: PathBuf,
    state_db: &'a StateDb,
}

impl<'a> ThreadStore<'a> {
    pub fn list_recent_threads_for_db(
        state_db: &StateDb,
        limit: usize,
    ) -> Result<Vec<ThreadSummary>> {
        state_db
            .list_recent_thread_records(limit)
            .map(|threads| threads.into_iter().map(ThreadSummary::from).collect())
    }

    pub fn new(session_manager: &'a SessionManager, state_db: &'a StateDb) -> Self {
        Self::new_for_roots(
            session_manager.storage_dir.clone(),
            session_manager.legacy_storage_dir.clone(),
            state_db,
        )
    }

    pub fn new_for_roots(
        rollout_root: PathBuf,
        legacy_session_root: PathBuf,
        state_db: &'a StateDb,
    ) -> Self {
        Self {
            rollout_root,
            legacy_session_root,
            state_db,
        }
    }

    pub fn latest_thread_id(&self) -> Result<Option<String>> {
        Ok(self
            .latest_thread_summary()?
            .map(|thread| thread.metadata.session_id))
    }

    pub fn latest_thread_summary(&self) -> Result<Option<ThreadSummary>> {
        Ok(self.list_recent_threads(1)?.into_iter().next())
    }

    pub fn list_recent_threads(&self, limit: usize) -> Result<Vec<ThreadSummary>> {
        Self::list_recent_threads_for_db(self.state_db, limit)
    }

    pub fn load_thread(&self, session_id: &str) -> Result<ThreadSnapshot> {
        let materialized = self.materialize_thread_state(session_id)?;
        Ok(ThreadSnapshot {
            metadata: materialized.metadata,
            provenance: materialized.provenance,
            history: materialized.history,
            compaction: materialized.compaction,
            plan_explanation: materialized.plan_explanation,
            plan_steps: materialized.plan_steps,
            interactions: materialized.interactions,
            rollout_items: materialized.rollout_items,
        })
    }

    pub fn export_thread_markdown(&self, session_id: &str) -> Result<String> {
        Ok(format_thread_markdown(&self.load_thread(session_id)?))
    }

    pub async fn distill_thread_summary(
        &self,
        memory_store: &MemoryStore,
        session_id: &str,
    ) -> Result<Option<MemoryRecord>> {
        let snapshot = self.load_thread(session_id)?;
        let Some(input) = thread_summary_memory_record(&snapshot) else {
            return Ok(None);
        };
        Ok(Some(memory_store.insert(input).await?))
    }

    pub async fn distill_thread_memories(
        &self,
        memory_store: &MemoryStore,
        session_id: &str,
    ) -> Result<Vec<MemoryRecord>> {
        let snapshot = self.load_thread(session_id)?;
        let markdown = format_thread_markdown(&snapshot);
        let distiller = MemoryDistiller::new(memory_store.backend());
        let drafts = distiller.distill_thread_markdown(&markdown).await?;
        if drafts.is_empty() {
            return Ok(Vec::new());
        }

        let mut existing_hits = Vec::with_capacity(drafts.len());
        for draft in &drafts {
            existing_hits.push(memory_store.search(&draft.content, 5).await?);
        }

        let base = thread_distilled_memory_base(&snapshot)?;
        let mut records = Vec::new();
        for draft in dedupe_memory_drafts(drafts, &existing_hits) {
            records.push(
                memory_store
                    .insert(new_memory_record_from_draft(draft, base.clone()))
                    .await?,
            );
        }
        Ok(records)
    }

    pub fn fork_thread(&self, source_thread_id: &str) -> Result<String> {
        let materialized = self.materialize_thread_state(source_thread_id)?;
        let runtime_state = self
            .state_db
            .load_session_runtime_state(source_thread_id)?
            .unwrap_or_default();
        let compact_state = compact_state_from_record(&materialized.compaction);
        let forked_thread_id = Uuid::new_v4().to_string();
        let lineage = PersistedThreadLineage {
            origin_kind: "fork".to_string(),
            forked_from_thread_id: Some(source_thread_id.to_string()),
        };

        let recorder = ThreadRecorder::new(self.state_db);
        recorder.persist_history_checkpoint(&forked_thread_id, &materialized.history)?;
        recorder.persist_runtime_state_with_lineage(
            &ThreadRuntimeState {
                session_id: &forked_thread_id,
                cwd: &materialized.metadata.cwd,
                branch: &materialized.metadata.branch,
                provider: &materialized.metadata.provider,
                model: &materialized.metadata.model,
                base_url: materialized.metadata.base_url.as_deref(),
                agent_mode: &materialized.metadata.agent_mode,
                bash_approval: &materialized.metadata.bash_approval,
                plan_explanation: materialized.plan_explanation.as_deref(),
                prompt_runtime: runtime_state.prompt_runtime.clone(),
                history_len: materialized.history.len(),
                transcript_len: materialized.metadata.transcript_len,
                compact_state: compact_state.clone(),
            },
            &ThreadRuntimeLineage {
                origin_kind: lineage.origin_kind.clone(),
                forked_from_thread_id: lineage.forked_from_thread_id.clone(),
            },
        )?;
        recorder.replace_plan_steps(&forked_thread_id, &materialized.plan_steps)?;
        recorder.replace_interactions(&forked_thread_id, &materialized.interactions)?;

        for compaction in self.load_compaction_events(source_thread_id)? {
            recorder.persist_compaction_event(&forked_thread_id, &compaction)?;
        }

        recorder.replace_runtime_rollout_events(
            &forked_thread_id,
            &[PersistedStructuredRolloutEvent::RuntimeState {
                recorded_at: None,
                explanation: materialized.plan_explanation.clone(),
                steps: materialized.plan_steps.clone(),
                interactions: materialized.interactions.clone(),
            }],
        )?;

        for item in self.load_turn_items(source_thread_id)? {
            recorder.persist_turn(&forked_thread_id, item.summary.ordinal, &item.entries)?;
        }

        Ok(forked_thread_id)
    }

    fn materialize_thread_state(&self, session_id: &str) -> Result<ThreadMaterializedState> {
        let (history, history_source) = match self.load_thread_history_migration(session_id) {
            Ok(migration) => (
                migration.history,
                match migration.source {
                    PersistedThreadHistorySource::Transcript => {
                        ThreadHistorySource::CanonicalHistory
                    }
                    PersistedThreadHistorySource::SnapshotBackfilled => {
                        ThreadHistorySource::HistorySnapshotBackfilled
                    }
                    PersistedThreadHistorySource::LegacyBackfilled => {
                        ThreadHistorySource::LegacyHistoryBackfilled
                    }
                },
            ),
            Err(err) if SessionManager::is_missing_thread_history_error(&err) => {
                (Vec::new(), ThreadHistorySource::Missing)
            }
            Err(err) => return Err(err),
        };
        let (metadata_record, metadata_source) = self.load_thread_metadata(session_id)?;
        if metadata_record.is_none() {
            anyhow::bail!("Thread {session_id} not found in thread metadata");
        }
        let metadata_record = metadata_record.expect("metadata record checked above");
        let mut plan_explanation = metadata_record.plan_explanation.clone();
        let metadata = ThreadMetadata::from(metadata_record);
        let LegacyNonTurnRolloutMigration {
            structured_events,
            runtime_rollout: migration_runtime_rollout,
            compaction_events,
            rollout_source,
            compaction_source,
        } = self.load_legacy_non_turn_rollout_migration(session_id)?;
        self.state_db
            .sync_spawn_agent_edges_from_events(session_id, &structured_events)?;
        let had_structured_events = !structured_events.is_empty();
        let had_compaction_events = !compaction_events.is_empty();
        let compaction = compaction_events
            .last()
            .cloned()
            .map(CompactionRecord::from)
            .unwrap_or_else(|| {
                self.state_db
                    .load_session_compact_state(session_id)
                    .map(CompactionRecord::from)
                    .unwrap_or_default()
            });
        let mut plan_steps = self.state_db.load_plan_steps(session_id)?;
        let mut interactions = self.state_db.load_interactions(session_id)?;
        let turn_items = self.load_turn_items(session_id)?;
        let mut rollout_items = Vec::new();
        let mut ordered_rollout_items = Vec::new();
        let mut rollout_order = 0usize;
        if structured_events.is_empty() && compaction.compaction_count > 0 {
            push_rollout_item(
                &mut ordered_rollout_items,
                &mut rollout_order,
                0,
                RolloutItem::Compaction(compaction.clone()),
            );
        }
        let mut saw_runtime_state = false;
        let mut saw_plan_state = false;
        let mut saw_interaction = false;
        let mut structured_plan_explanation = None;
        let mut structured_plan_steps = Vec::new();
        let mut structured_interactions = Vec::new();
        let mut latest_runtime_state = None;
        for item in structured_events {
            match item {
                PersistedStructuredRolloutEvent::Compaction {
                    recorded_at,
                    event_index,
                    before_tokens,
                    after_tokens,
                    boundary_version,
                    replaced_start,
                    replaced_end,
                    metadata_owner,
                    recent_files,
                    summary,
                } => push_rollout_item(
                    &mut ordered_rollout_items,
                    &mut rollout_order,
                    recorded_at.unwrap_or(0),
                    RolloutItem::Compaction(CompactionRecord {
                        compaction_count: event_index,
                        before_tokens: Some(before_tokens),
                        after_tokens: Some(after_tokens),
                        recent_file_count: Some(recent_files.len()),
                        boundary_version: Some(boundary_version),
                        replaced_start,
                        replaced_end,
                        metadata_owner,
                        recent_files,
                        summary: Some(summary),
                    }),
                ),
                PersistedStructuredRolloutEvent::RuntimeState {
                    recorded_at,
                    explanation,
                    steps,
                    interactions: runtime_interactions,
                } => {
                    saw_runtime_state = true;
                    saw_plan_state = true;
                    saw_interaction = true;
                    plan_explanation = explanation.clone();
                    plan_steps = steps.clone();
                    interactions = runtime_interactions.clone();
                    let base_order = rollout_order;
                    let reserved_items = usize::from(!steps.is_empty() || explanation.is_some())
                        + runtime_interactions.len();
                    rollout_order += reserved_items.max(1);
                    latest_runtime_state = Some((
                        recorded_at.unwrap_or(0),
                        base_order,
                        explanation,
                        steps,
                        runtime_interactions,
                    ));
                }
                PersistedStructuredRolloutEvent::PlanState {
                    recorded_at,
                    explanation,
                    steps,
                } => {
                    saw_plan_state = true;
                    structured_plan_explanation = explanation.clone();
                    structured_plan_steps = steps.clone();
                    push_rollout_item(
                        &mut ordered_rollout_items,
                        &mut rollout_order,
                        recorded_at.unwrap_or(0),
                        RolloutItem::PlanState { explanation, steps },
                    );
                }
                PersistedStructuredRolloutEvent::Interaction {
                    recorded_at,
                    interaction,
                } => {
                    saw_interaction = true;
                    structured_interactions.push(interaction.clone());
                    push_rollout_item(
                        &mut ordered_rollout_items,
                        &mut rollout_order,
                        recorded_at.unwrap_or(0),
                        RolloutItem::Interaction(interaction),
                    );
                }
                PersistedStructuredRolloutEvent::SpawnAgent {
                    recorded_at,
                    event_id,
                    agent_id,
                    name,
                    child_session_id,
                    status,
                    summary,
                } => {
                    push_rollout_item(
                        &mut ordered_rollout_items,
                        &mut rollout_order,
                        recorded_at.unwrap_or(0),
                        RolloutItem::SpawnAgent {
                            event_id,
                            agent_id,
                            name,
                            child_session_id,
                            status,
                            summary,
                        },
                    );
                }
            }
        }

        if let Some((recorded_at, base_order, explanation, steps, runtime_interactions)) =
            latest_runtime_state
        {
            let mut item_order = base_order;
            if !steps.is_empty() || explanation.is_some() {
                ordered_rollout_items.push((
                    recorded_at,
                    item_order,
                    RolloutItem::PlanState { explanation, steps },
                ));
                item_order += 1;
            }
            for interaction in runtime_interactions {
                ordered_rollout_items.push((
                    recorded_at,
                    item_order,
                    RolloutItem::Interaction(interaction),
                ));
                item_order += 1;
            }
        }

        if !saw_runtime_state {
            if saw_plan_state {
                plan_explanation = structured_plan_explanation;
                plan_steps = structured_plan_steps;
            }
            if saw_interaction {
                interactions = structured_interactions;
            }
        }

        let legacy_runtime_rollout = if saw_runtime_state || (saw_plan_state && saw_interaction) {
            Vec::new()
        } else {
            migration_runtime_rollout
        };
        let has_legacy_runtime_rollout = !legacy_runtime_rollout.is_empty();
        let legacy_plan_state = legacy_runtime_rollout.iter().find_map(|item| match item {
            PersistedRuntimeRolloutItem::PlanState { explanation, steps } => {
                Some((explanation.clone(), steps.clone()))
            }
            PersistedRuntimeRolloutItem::Interaction(_) => None,
        });
        let legacy_interactions = legacy_runtime_rollout
            .iter()
            .filter_map(|item| match item {
                PersistedRuntimeRolloutItem::Interaction(interaction) => Some(interaction.clone()),
                PersistedRuntimeRolloutItem::PlanState { .. } => None,
            })
            .collect::<Vec<_>>();

        if saw_runtime_state {
            // Append-only runtime snapshots already defined the current plan/interaction state.
        } else if !saw_plan_state && !saw_interaction && legacy_runtime_rollout.is_empty() {
            if !plan_steps.is_empty() || plan_explanation.is_some() {
                push_rollout_item(
                    &mut ordered_rollout_items,
                    &mut rollout_order,
                    0,
                    RolloutItem::PlanState {
                        explanation: plan_explanation.clone(),
                        steps: plan_steps.clone(),
                    },
                );
            }
            for interaction in interactions.iter().cloned() {
                push_rollout_item(
                    &mut ordered_rollout_items,
                    &mut rollout_order,
                    0,
                    RolloutItem::Interaction(interaction),
                );
            }
        } else if !saw_plan_state && !saw_interaction {
            if let Some((explanation, steps)) = legacy_plan_state.clone() {
                plan_explanation = explanation;
                plan_steps = steps;
            }
            interactions = legacy_interactions.clone();
            for item in legacy_runtime_rollout.iter().cloned() {
                push_rollout_item(
                    &mut ordered_rollout_items,
                    &mut rollout_order,
                    0,
                    match item {
                        PersistedRuntimeRolloutItem::PlanState { explanation, steps } => {
                            RolloutItem::PlanState { explanation, steps }
                        }
                        PersistedRuntimeRolloutItem::Interaction(interaction) => {
                            RolloutItem::Interaction(interaction)
                        }
                    },
                );
            }
        } else {
            if !saw_plan_state {
                if legacy_runtime_rollout.is_empty() {
                    if !plan_steps.is_empty() || plan_explanation.is_some() {
                        push_rollout_item(
                            &mut ordered_rollout_items,
                            &mut rollout_order,
                            0,
                            RolloutItem::PlanState {
                                explanation: plan_explanation.clone(),
                                steps: plan_steps.clone(),
                            },
                        );
                    }
                } else {
                    if let Some((explanation, steps)) = legacy_plan_state.clone() {
                        plan_explanation = explanation;
                        plan_steps = steps;
                    }
                    for item in legacy_runtime_rollout.iter() {
                        if let PersistedRuntimeRolloutItem::PlanState { explanation, steps } = item
                        {
                            push_rollout_item(
                                &mut ordered_rollout_items,
                                &mut rollout_order,
                                0,
                                RolloutItem::PlanState {
                                    explanation: explanation.clone(),
                                    steps: steps.clone(),
                                },
                            );
                        }
                    }
                }
            }
            if !saw_interaction {
                if legacy_runtime_rollout.is_empty() {
                    for interaction in interactions.iter().cloned() {
                        push_rollout_item(
                            &mut ordered_rollout_items,
                            &mut rollout_order,
                            0,
                            RolloutItem::Interaction(interaction),
                        );
                    }
                } else {
                    interactions = legacy_interactions.clone();
                    for item in legacy_runtime_rollout.iter().cloned() {
                        if let PersistedRuntimeRolloutItem::Interaction(interaction) = item {
                            push_rollout_item(
                                &mut ordered_rollout_items,
                                &mut rollout_order,
                                0,
                                RolloutItem::Interaction(interaction),
                            );
                        }
                    }
                }
            }
        }
        for item in turn_items {
            let timestamp = item.summary.updated_at;
            push_rollout_item(
                &mut ordered_rollout_items,
                &mut rollout_order,
                timestamp,
                RolloutItem::Turn(item),
            );
        }
        ordered_rollout_items.sort_by_key(|(timestamp, order, _)| (*timestamp, *order));
        rollout_items.extend(ordered_rollout_items.into_iter().map(|(_, _, item)| item));

        let used_state_db_non_turn_fallback = (!saw_plan_state
            && !saw_interaction
            && !has_legacy_runtime_rollout
            && (plan_explanation.is_some() || !plan_steps.is_empty() || !interactions.is_empty()))
            || (!had_structured_events
                && !had_compaction_events
                && compaction.compaction_count > 0);
        let non_turn_rollout_source = if matches!(
            rollout_source,
            PersistedLegacyRolloutSource::LegacyBackfilled
        ) || matches!(
            compaction_source,
            PersistedCompactionEventsSource::LegacyBackfilled
        ) {
            ThreadNonTurnRolloutSource::LegacyBackfilled
        } else if had_structured_events
            || matches!(
                compaction_source,
                PersistedCompactionEventsSource::StructuredLog
            )
        {
            ThreadNonTurnRolloutSource::StructuredEventsLog
        } else if used_state_db_non_turn_fallback {
            ThreadNonTurnRolloutSource::StateDbFallback
        } else {
            ThreadNonTurnRolloutSource::Empty
        };

        Ok(ThreadMaterializedState {
            metadata,
            provenance: ThreadMaterializationProvenance {
                metadata_source,
                history_source,
                non_turn_rollout_source,
            },
            history,
            compaction,
            plan_explanation,
            plan_steps,
            interactions,
            rollout_items,
        })
    }

    fn load_legacy_non_turn_rollout_migration(
        &self,
        session_id: &str,
    ) -> Result<LegacyNonTurnRolloutMigration> {
        let PersistedLegacyRolloutMigration {
            structured_events,
            runtime_rollout,
            source,
        } = self.state_db.load_legacy_rollout_migration(session_id)?;
        let compaction_events = self.load_compaction_events_migration(session_id)?;
        Ok(LegacyNonTurnRolloutMigration {
            structured_events,
            runtime_rollout,
            compaction_events: compaction_events.events,
            rollout_source: source,
            compaction_source: compaction_events.source,
        })
    }

    fn load_turn_items(&self, session_id: &str) -> Result<Vec<RolloutTurnItem>> {
        let turn_records = thread_turn_log::load_turn_records(&self.rollout_root, session_id)?;
        let mut by_ordinal = BTreeMap::new();
        for summary in self.state_db.load_turn_summaries(session_id)? {
            let entries = self
                .state_db
                .load_turn_entries(session_id, summary.ordinal)?;
            by_ordinal.insert(summary.ordinal, RolloutTurnItem { summary, entries });
        }
        for record in turn_records {
            by_ordinal.insert(
                record.summary.ordinal,
                RolloutTurnItem {
                    summary: record.summary,
                    entries: record.entries,
                },
            );
        }
        Ok(by_ordinal.into_values().collect())
    }

    fn load_thread_history_migration(
        &self,
        thread_id: &str,
    ) -> Result<PersistedThreadHistoryMigration> {
        let transcript_path =
            session_transcript::main_transcript_path(&self.rollout_root, thread_id);
        if transcript_path.exists() {
            match session_transcript::load_transcript(&transcript_path) {
                Ok(load) => {
                    let history = session_transcript::model_visible_messages(&load.entries);
                    let has_snapshot_fallback = self.session_history_path(thread_id).exists()
                        || self.legacy_session_history_path(thread_id).exists();
                    let should_use_fallback = has_snapshot_fallback
                        && (history.is_empty()
                            || load.parse_errors > 0
                            || self
                                .transcript_is_shorter_than_snapshot_prefix(thread_id, &history)
                                .unwrap_or(false));
                    if !should_use_fallback && (load.parse_errors == 0 || !has_snapshot_fallback) {
                        return Ok(PersistedThreadHistoryMigration {
                            history,
                            source: PersistedThreadHistorySource::Transcript,
                        });
                    }
                }
                Err(err)
                    if !self.session_history_path(thread_id).exists()
                        && !self.legacy_session_history_path(thread_id).exists() =>
                {
                    return Err(err).context("load canonical session transcript");
                }
                Err(err) => {
                    eprintln!(
                        "Warning: could not load canonical session transcript for {thread_id}: {err}"
                    );
                }
            }
        }

        let path = self.session_history_path(thread_id);
        let (history, source) = if path.exists() {
            let content = fs::read_to_string(path)?;
            let history: Vec<Message> = serde_json::from_str(&content)?;
            let _ =
                session_transcript::write_history_snapshot(&self.rollout_root, thread_id, &history);
            (history, PersistedThreadHistorySource::SnapshotBackfilled)
        } else {
            let legacy = self.legacy_session_history_path(thread_id);
            if !legacy.exists() {
                return Err(anyhow!("Thread not found locally"));
            }
            let content = fs::read_to_string(&legacy)?;
            let history: Vec<Message> = serde_json::from_str(&content)?;
            self.backfill_legacy_thread_history(thread_id, &history)?;
            (history, PersistedThreadHistorySource::LegacyBackfilled)
        };
        Ok(PersistedThreadHistoryMigration { history, source })
    }

    fn load_compaction_events(&self, session_id: &str) -> Result<Vec<PersistedCompactionEvent>> {
        Ok(self.load_compaction_events_migration(session_id)?.events)
    }

    fn load_compaction_events_migration(
        &self,
        session_id: &str,
    ) -> Result<PersistedCompactionEventsMigration> {
        let events = self.load_structured_rollout_events(session_id)?;
        let structured_compactions = events
            .into_iter()
            .filter_map(|event| match event {
                PersistedStructuredRolloutEvent::Compaction {
                    recorded_at: _,
                    event_index,
                    before_tokens,
                    after_tokens,
                    boundary_version,
                    replaced_start,
                    replaced_end,
                    metadata_owner,
                    recent_files,
                    summary,
                } => Some(PersistedCompactionEvent {
                    event_index,
                    before_tokens,
                    after_tokens,
                    boundary_version,
                    replaced_start,
                    replaced_end,
                    metadata_owner,
                    recent_files,
                    summary,
                }),
                PersistedStructuredRolloutEvent::RuntimeState { .. }
                | PersistedStructuredRolloutEvent::PlanState { .. }
                | PersistedStructuredRolloutEvent::Interaction { .. }
                | PersistedStructuredRolloutEvent::SpawnAgent { .. } => None,
            })
            .collect::<Vec<_>>();
        if !structured_compactions.is_empty() {
            return Ok(PersistedCompactionEventsMigration {
                events: structured_compactions,
                source: PersistedCompactionEventsSource::StructuredLog,
            });
        }
        let path = self.session_compaction_events_path(session_id);
        if !path.exists() {
            return Ok(PersistedCompactionEventsMigration {
                events: Vec::new(),
                source: PersistedCompactionEventsSource::Empty,
            });
        }
        let content = fs::read_to_string(path)?;
        let compactions: Vec<PersistedCompactionEvent> = serde_json::from_str(&content)?;
        self.backfill_legacy_compaction_events(session_id, &compactions)?;
        Ok(PersistedCompactionEventsMigration {
            events: compactions,
            source: PersistedCompactionEventsSource::LegacyBackfilled,
        })
    }

    fn load_structured_rollout_events(
        &self,
        session_id: &str,
    ) -> Result<Vec<PersistedStructuredRolloutEvent>> {
        thread_rollout_log::load_rollout_events(&self.rollout_root, session_id)
    }

    fn session_history_path(&self, session_id: &str) -> PathBuf {
        self.rollout_root.join(session_id).join("history.json")
    }

    fn legacy_session_history_path(&self, session_id: &str) -> PathBuf {
        self.legacy_session_root.join(format!("{session_id}.json"))
    }

    fn session_compaction_events_path(&self, session_id: &str) -> PathBuf {
        self.rollout_root.join(session_id).join("compactions.json")
    }

    fn backfill_legacy_thread_history(&self, thread_id: &str, history: &[Message]) -> Result<()> {
        if history.is_empty() {
            return Ok(());
        }
        let path = self.session_history_path(thread_id);
        if path.exists() {
            return Ok(());
        }
        write_history_snapshot(&self.rollout_root, thread_id, history)?;
        let _ = session_transcript::write_history_snapshot(&self.rollout_root, thread_id, history);
        Ok(())
    }

    fn backfill_legacy_compaction_events(
        &self,
        session_id: &str,
        compactions: &[PersistedCompactionEvent],
    ) -> Result<()> {
        if compactions.is_empty() {
            return Ok(());
        }
        let rollout_path =
            thread_rollout_log::rollout_events_log_path(&self.rollout_root, session_id);
        let existing_compaction_count = if rollout_path.exists() {
            self.load_structured_rollout_events(session_id)?
                .into_iter()
                .filter(|event| matches!(event, PersistedStructuredRolloutEvent::Compaction { .. }))
                .count()
        } else {
            0
        };
        if existing_compaction_count >= compactions.len() {
            return Ok(());
        }

        for compaction in compactions.iter().skip(existing_compaction_count) {
            thread_rollout_log::append_rollout_event_line(
                &self.rollout_root,
                session_id,
                &PersistedStructuredRolloutEvent::Compaction {
                    recorded_at: None,
                    event_index: compaction.event_index,
                    before_tokens: compaction.before_tokens,
                    after_tokens: compaction.after_tokens,
                    boundary_version: compaction.boundary_version,
                    replaced_start: compaction.replaced_start,
                    replaced_end: compaction.replaced_end,
                    metadata_owner: compaction.metadata_owner.clone(),
                    recent_files: compaction.recent_files.clone(),
                    summary: compaction.summary.clone(),
                },
            )?;
        }
        Ok(())
    }

    fn transcript_is_shorter_than_snapshot_prefix(
        &self,
        thread_id: &str,
        transcript_history: &[Message],
    ) -> Result<bool> {
        let snapshot_path = self.session_history_path(thread_id);
        if !snapshot_path.exists() {
            return Ok(false);
        }
        let content = fs::read_to_string(snapshot_path)?;
        let snapshot_history: Vec<Message> = serde_json::from_str(&content)?;
        Ok(transcript_history.len() < snapshot_history.len()
            && snapshot_history.starts_with(transcript_history))
    }

    fn load_thread_metadata(
        &self,
        session_id: &str,
    ) -> Result<(Option<PersistedThreadRecord>, ThreadMetadataSource)> {
        if let Some(record) = thread_metadata::load_thread_record(&self.rollout_root, session_id)? {
            return Ok((Some(record), ThreadMetadataSource::StructuredMetadata));
        }
        Ok((
            self.state_db.load_thread_record(session_id)?,
            ThreadMetadataSource::StateDb,
        ))
    }
}

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

fn format_thread_markdown(thread: &ThreadSnapshot) -> String {
    let title = thread_title(thread);
    let mut lines = vec![
        "---".to_string(),
        format!("title: {}", yaml_scalar(&title)),
        "source: rara".to_string(),
        format!("session_id: {}", yaml_scalar(&thread.metadata.session_id)),
        format!("workspace: {}", yaml_scalar(&thread.metadata.cwd)),
        format!("model: {}", yaml_scalar(&thread.metadata.model)),
        format!("created_at: {}", thread.metadata.created_at),
        format!("updated_at: {}", thread.metadata.updated_at),
        "---".to_string(),
        String::new(),
        format!("# {title}"),
        String::new(),
    ];

    if let Some(summary) = thread
        .compaction
        .summary
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        lines.push("## Summary".to_string());
        lines.push(summary.trim().to_string());
        lines.push(String::new());
    }

    for message in &thread.history {
        lines.push(format!("## {}", role_heading(&message.role)));
        lines.push(render_message_content(&message.content));
        lines.push(String::new());
    }

    lines.join("\n")
}

fn thread_summary_memory_record(thread: &ThreadSnapshot) -> Option<NewMemoryRecord> {
    let summary = thread
        .compaction
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| first_user_message(thread));
    let summary = summary?;
    let title = thread_title(thread);
    let mut record = thread_distilled_memory_base(thread).ok()?;
    record.title = Some(title);
    record.content = format!(
        concat!(
            "## Summary\n",
            "{summary}\n\n",
            "## Provenance\n",
            "- session_id: {session_id}\n",
            "- thread_id: {thread_id}\n",
            "- workspace: {workspace}\n",
            "- provider: {provider}\n",
            "- model: {model}\n"
        ),
        summary = summary.as_str(),
        session_id = thread.metadata.session_id.as_str(),
        thread_id = thread.metadata.session_id.as_str(),
        workspace = thread.metadata.cwd.as_str(),
        provider = thread.metadata.provider.as_str(),
        model = thread.metadata.model.as_str(),
    );
    Some(record)
}

fn thread_distilled_memory_base(thread: &ThreadSnapshot) -> Result<NewMemoryRecord> {
    let end_turn_index = thread.history.len().saturating_sub(1) as u32;
    NewMemoryRecord::promotion_base(
        MemorySource::ThreadDistill,
        MemoryPromotionTarget::Thread {
            session_id: Some(thread.metadata.session_id.clone()),
            thread_id: thread.metadata.session_id.clone(),
            source_span: Some(MemorySourceSpan {
                start_turn_index: 0,
                end_turn_index,
            }),
        },
    )
}

fn thread_title(thread: &ThreadSnapshot) -> String {
    first_user_message(thread)
        .map(|value| truncate_chars(&collapse_whitespace(&value), 80))
        .unwrap_or_else(|| format!("Thread {}", thread.metadata.session_id))
}

fn first_user_message(thread: &ThreadSnapshot) -> Option<String> {
    thread
        .history
        .iter()
        .find(|message| message.role == "user")
        .map(|message| render_message_content(&message.content))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn role_heading(role: &str) -> &str {
    match role {
        "assistant" => "Assistant",
        "system" => "System",
        "tool" => "Tool",
        "user" => "User",
        _ => "Message",
    }
}

fn render_message_content(content: &serde_json::Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    if let Some(items) = content.as_array() {
        let rendered = items
            .iter()
            .filter_map(render_content_item)
            .collect::<Vec<_>>();
        if !rendered.is_empty() {
            return rendered.join("\n\n");
        }
    }
    serde_json::to_string_pretty(content).unwrap_or_else(|_| content.to_string())
}

fn render_content_item(item: &serde_json::Value) -> Option<String> {
    match item.get("type").and_then(serde_json::Value::as_str) {
        Some("text") => item
            .get("text")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        Some("tool_use") => {
            let name = item
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("tool");
            let input = item
                .get("input")
                .map(|value| {
                    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
                })
                .unwrap_or_else(|| "{}".to_string());
            Some(format!("Tool use: {name}\n\n```json\n{input}\n```"))
        }
        Some("tool_result") => item
            .get("content")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        _ => Some(serde_json::to_string_pretty(item).unwrap_or_else(|_| item.to_string())),
    }
}

fn yaml_scalar(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

fn compact_state_from_record(record: &CompactionRecord) -> PersistedCompactState {
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

fn write_history_snapshot(root_dir: &Path, session_id: &str, history: &[Message]) -> Result<()> {
    let path = root_dir.join(session_id).join("history.json");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string(history)?;
    let tmp_path = path.with_extension(format!("json.tmp-{}", Uuid::new_v4()));
    {
        let mut file = fs::File::create(&tmp_path)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
    }
    if let Err(err) = atomic_file::replace_file(&tmp_path, &path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }
    if let Some(parent) = path.parent() {
        sync_parent_dir_best_effort(parent);
    }
    Ok(())
}

#[cfg(unix)]
fn sync_parent_dir_best_effort(parent: &Path) {
    if let Ok(dir) = fs::File::open(parent) {
        let _ = dir.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_parent_dir_best_effort(_parent: &Path) {}

fn push_rollout_item(
    ordered_items: &mut Vec<(i64, usize, RolloutItem)>,
    rollout_order: &mut usize,
    timestamp: i64,
    item: RolloutItem,
) {
    ordered_items.push((timestamp, *rollout_order, item));
    *rollout_order += 1;
}
