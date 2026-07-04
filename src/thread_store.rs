use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use rara_persistence::atomic_file;
use rara_persistence::thread_data::{
    PersistedCompactState, PersistedInteraction, PersistedLegacyRolloutMigration,
    PersistedLegacyRolloutSource, PersistedPlanStep, PersistedRuntimeRolloutItem,
    PersistedStructuredRolloutEvent, PersistedThreadLineage, PersistedThreadRecord,
};
use rara_persistence::thread_metadata;
use rara_persistence::thread_rollout_log;
use rara_persistence::thread_turn_log;
use rara_state::state_db::StateDb;
use uuid::Uuid;

use crate::agent::Message;
use crate::memory_distiller::{
    MemoryDistiller, dedupe_memory_drafts, new_memory_record_from_draft,
};
use crate::memory_store::{MemoryRecord, MemoryStore};
use crate::session::{
    PersistedCompactionEvent, PersistedCompactionEventsMigration, PersistedCompactionEventsSource,
    PersistedThreadHistoryMigration, PersistedThreadHistorySource, SessionManager,
};
use crate::session_transcript;

mod format;
mod recorder;
mod types;

#[cfg(test)]
mod tests;

pub use recorder::{ThreadRecorder, ThreadRuntimeLineage, ThreadRuntimeState};
pub use types::{
    CompactionRecord, RolloutItem, RolloutTurnItem, ThreadHistorySource,
    ThreadMaterializationProvenance, ThreadMetadata, ThreadMetadataSource,
    ThreadNonTurnRolloutSource, ThreadSnapshot, ThreadSummary,
};

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

    /// Reserved portable export boundary for external thread inspection.
    /// This is part of the thread contract documented in docs/features/threads.md.
    #[allow(dead_code)]
    pub fn export_thread_markdown(&self, session_id: &str) -> Result<String> {
        Ok(format::format_thread_markdown(
            &self.load_thread(session_id)?,
        ))
    }

    /// Reserved compatibility path for one-record thread distillation.
    /// The active product path is `distill_thread_memories`; this remains part
    /// of the thread contract documented in docs/features/memory-records.md.
    #[allow(dead_code)]
    pub async fn distill_thread_summary(
        &self,
        memory_store: &MemoryStore,
        session_id: &str,
    ) -> Result<Option<MemoryRecord>> {
        let snapshot = self.load_thread(session_id)?;
        let Some(input) = format::thread_summary_memory_record(&snapshot) else {
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
        let markdown = format::format_thread_markdown(&snapshot);
        let distiller = MemoryDistiller::new(memory_store.backend());
        let drafts = distiller.distill_thread_markdown(&markdown).await?;
        if drafts.is_empty() {
            return Ok(Vec::new());
        }

        let mut existing_hits = Vec::with_capacity(drafts.len());
        for draft in &drafts {
            existing_hits.push(memory_store.search(&draft.content, 5).await?);
        }

        let base = format::thread_distilled_memory_base(&snapshot)?;
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
        let compact_state = recorder::compact_state_from_record(&materialized.compaction);
        let plan_lifecycle = materialized
            .rollout_items
            .iter()
            .filter_map(|item| match item {
                RolloutItem::PlanLifecycle(lifecycle) => Some(lifecycle.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
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
                plan_lifecycle,
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
                    ..
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
                    plan_lifecycle,
                } => {
                    saw_runtime_state = true;
                    saw_plan_state = true;
                    saw_interaction = true;
                    plan_explanation = explanation.clone();
                    plan_steps = steps.clone();
                    interactions = runtime_interactions.clone();
                    let base_order = rollout_order;
                    let reserved_items = usize::from(!steps.is_empty() || explanation.is_some())
                        + runtime_interactions.len()
                        + plan_lifecycle.len();
                    rollout_order += reserved_items.max(1);
                    latest_runtime_state = Some((
                        recorded_at.unwrap_or(0),
                        base_order,
                        explanation,
                        steps,
                        runtime_interactions,
                        plan_lifecycle,
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
                PersistedStructuredRolloutEvent::PlanLifecycle {
                    recorded_at,
                    lifecycle,
                } => {
                    push_rollout_item(
                        &mut ordered_rollout_items,
                        &mut rollout_order,
                        recorded_at.unwrap_or(0),
                        RolloutItem::PlanLifecycle(lifecycle),
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
                    ..
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

        if let Some((
            recorded_at,
            base_order,
            explanation,
            steps,
            runtime_interactions,
            plan_lifecycle,
        )) = latest_runtime_state
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
            for lifecycle in plan_lifecycle {
                ordered_rollout_items.push((
                    recorded_at,
                    item_order,
                    RolloutItem::PlanLifecycle(lifecycle),
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
                    ..
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
                | PersistedStructuredRolloutEvent::PlanLifecycle { .. }
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
