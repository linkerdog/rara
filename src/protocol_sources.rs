//! Protocol source registries.
//!
//! Stores and manages prompt sources, skill sources, and memory records
//! that are registered by external protocol adapters (ACP, Wire, etc.)
//! through the control plane. These sources enter normal precedence
//! resolution and prompt assembly alongside local sources.
//!
//! ## Design
//!
//! - Prompts: stored with provenance; transient (turn-limited) sources
//!   expire automatically; persistent sources need explicit unregistration.
//! - Skills: delegate to the local skill resolution path, with protocol
//!   origin recorded for precedence and override reporting.
//! - Memory: protocol-registered records are treated as normal memory
//!   records with protocol provenance.

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::RwLock;

use crate::memory_store::{
    MemoryLabel, MemoryRecord, MemoryRecordPatch, MemoryScope as StoreMemoryScope, MemorySource,
    MemoryStore, NewMemoryRecord,
};
use crate::runtime_control::{
    MemoryControlRequest, MemoryEvent, MemoryLabelCountEvent, MemoryRecordControlPatch,
    MemoryRecordEventView, MemoryScope as ControlMemoryScope, PromptSourceControlRequest,
    PromptSourceEvent, PromptSourceLifetime, PromptSourceRegistration, RuntimeEvent,
    SkillSourceControlRequest,
};
use crate::runtime_event_bus::RuntimeEventBus;

// ── Prompt source registry ──────────────────────────────────────────────

/// Stored entry for a protocol-registered prompt source.
#[derive(Clone, Debug)]
struct PromptSourceEntry {
    registration: PromptSourceRegistration,
    /// Remaining turn count (only meaningful for `Turns` lifetime).
    remaining_turns: Option<u32>,
}

/// Registry for protocol-registered prompt sources.
pub struct PromptSourceRegistry {
    event_bus: Arc<RuntimeEventBus>,
    sources: RwLock<BTreeMap<String, PromptSourceEntry>>,
}

impl PromptSourceRegistry {
    pub fn new(event_bus: Arc<RuntimeEventBus>) -> Self {
        Self {
            event_bus,
            sources: RwLock::new(BTreeMap::new()),
        }
    }

    pub async fn handle_control(&self, request: &PromptSourceControlRequest) {
        match request {
            PromptSourceControlRequest::Register(registration) => {
                let mut sources = self.sources.write().await;
                let turns = match registration.lifetime {
                    PromptSourceLifetime::Turns(n) => Some(n),
                    PromptSourceLifetime::Session | PromptSourceLifetime::Persistent => None,
                };
                sources.insert(
                    registration.source_id.clone(),
                    PromptSourceEntry {
                        registration: registration.clone(),
                        remaining_turns: turns,
                    },
                );
                let _ = self.event_bus.publish_control(RuntimeEvent::PromptSource(
                    PromptSourceEvent::Registered {
                        source_id: registration.source_id.clone(),
                    },
                ));
            }
            PromptSourceControlRequest::Unregister { source_id } => {
                let removed = self.sources.write().await.remove(source_id).is_some();
                if removed {
                    let _ = self.event_bus.publish_control(RuntimeEvent::PromptSource(
                        PromptSourceEvent::Unregistered {
                            source_id: source_id.clone(),
                        },
                    ));
                }
            }
            PromptSourceControlRequest::QuerySources => {
                let sources = self.sources.read().await;
                let ids: Vec<String> = sources.keys().cloned().collect();
                for id in ids {
                    let _ = self.event_bus.publish_control(RuntimeEvent::PromptSource(
                        PromptSourceEvent::Registered { source_id: id },
                    ));
                }
            }
        }
    }

    /// Decrement remaining turns for turn-limited sources.
    /// Sources whose remaining turns reach 0 are removed.
    pub async fn advance_turn(&self) {
        let mut sources = self.sources.write().await;
        let mut expired = Vec::new();
        for (id, entry) in sources.iter_mut() {
            if let Some(ref mut remaining) = entry.remaining_turns {
                if *remaining == 0 {
                    expired.push(id.clone());
                } else {
                    *remaining -= 1;
                }
            }
        }
        for id in &expired {
            sources.remove(id);
            let _ = self.event_bus.publish_control(RuntimeEvent::PromptSource(
                PromptSourceEvent::Dropped {
                    source_id: id.clone(),
                    reason: "turn limit expired".into(),
                },
            ));
        }
    }

    /// Return all registered sources (for prompt assembly).
    pub async fn list_sources(&self) -> Vec<PromptSourceRegistration> {
        self.sources
            .read()
            .await
            .values()
            .map(|e| e.registration.clone())
            .collect()
    }
}

// ── Skill source registry ───────────────────────────────────────────────

/// Stored entry for a protocol-registered skill or skill root.
#[derive(Clone, Debug)]
pub struct SkillSourceEntry {
    pub source_id: String,
    pub precedence_hint: Option<i32>,
}

/// Registry for protocol-registered skill sources.
///
/// This is intentionally thin: it records protocol-origin metadata that
/// augments the local skill discovery path. Protocol skills enter the
/// same precedence/resolution as local `SKILL.md` files.
pub struct SkillSourceRegistry {
    event_bus: Arc<RuntimeEventBus>,
    /// Protocol-registered skill roots (path overrides).
    roots: RwLock<BTreeMap<String, SkillSourceEntry>>,
    /// Protocol-registered inline skills (name → entry).
    skills: RwLock<BTreeMap<String, SkillSourceEntry>>,
    /// Disabled skill names.
    disabled: RwLock<Vec<String>>,
}

impl SkillSourceRegistry {
    pub fn new(event_bus: Arc<RuntimeEventBus>) -> Self {
        Self {
            event_bus,
            roots: RwLock::new(BTreeMap::new()),
            skills: RwLock::new(BTreeMap::new()),
            disabled: RwLock::new(Vec::new()),
        }
    }

    pub async fn handle_control(&self, request: &SkillSourceControlRequest) {
        match request {
            SkillSourceControlRequest::RegisterRoot {
                source_id,
                root: _root,
                precedence_hint,
            } => {
                self.roots.write().await.insert(
                    source_id.clone(),
                    SkillSourceEntry {
                        source_id: source_id.clone(),
                        precedence_hint: *precedence_hint,
                    },
                );
            }
            SkillSourceControlRequest::RegisterSkill {
                source_id,
                name,
                content: _content,
                precedence_hint,
            } => {
                self.skills.write().await.insert(
                    name.clone(),
                    SkillSourceEntry {
                        source_id: source_id.clone(),
                        precedence_hint: *precedence_hint,
                    },
                );
            }
            SkillSourceControlRequest::DisableSkill {
                name,
                source_id: _source_id,
            } => {
                self.disabled.write().await.push(name.clone());
            }
            SkillSourceControlRequest::QuerySkills => {
                let roots: Vec<String> = self.roots.read().await.keys().cloned().collect();
                let skills: Vec<String> = self.skills.read().await.keys().cloned().collect();
                for source_id in roots.into_iter().chain(skills) {
                    let _ = self.event_bus.publish_control(RuntimeEvent::PromptSource(
                        PromptSourceEvent::Registered { source_id },
                    ));
                }
            }
        }
    }
}

// ── Memory control handler ──────────────────────────────────────────────

/// Handler for protocol-originated memory control requests.
pub struct MemoryControlHandler {
    event_bus: Arc<RuntimeEventBus>,
    memory_store: Arc<MemoryStore>,
}

impl MemoryControlHandler {
    pub fn new(event_bus: Arc<RuntimeEventBus>, memory_store: Arc<MemoryStore>) -> Self {
        Self {
            event_bus,
            memory_store,
        }
    }

    pub async fn handle_control(&self, request: &MemoryControlRequest) -> Result<()> {
        match request {
            MemoryControlRequest::AddRecord {
                memory_id,
                scope,
                content,
                metadata,
            } => {
                let record = self
                    .memory_store
                    .insert(new_record_from_control(
                        memory_id, *scope, content, metadata,
                    )?)
                    .await?;
                self.publish_memory_event(MemoryEvent::RecordAdded {
                    memory_id: record.id,
                });
            }
            MemoryControlRequest::UpdateRecord { memory_id, patch } => {
                let record = self
                    .memory_store
                    .update(memory_id, patch_from_control(patch)?)
                    .await
                    .with_context(|| format!("update memory record {memory_id}"))?;
                self.publish_memory_event(MemoryEvent::RecordUpdated {
                    memory_id: record.id,
                });
            }
            MemoryControlRequest::DeleteRecord { memory_id } => {
                if let Some(record) = self
                    .memory_store
                    .delete(memory_id)
                    .await
                    .with_context(|| format!("delete memory record {memory_id}"))?
                {
                    self.publish_memory_event(MemoryEvent::RecordDeleted {
                        memory_id: record.id,
                    });
                }
            }
            MemoryControlRequest::ListLabels { scope } => {
                let labels = self
                    .memory_store
                    .list_labels(scope.map(scope_from_control))
                    .await?
                    .into_iter()
                    .map(|entry| MemoryLabelCountEvent {
                        label: label_to_string(entry.label).to_string(),
                        count: entry.count,
                    })
                    .collect();
                self.publish_memory_event(MemoryEvent::LabelsListed { labels });
            }
            MemoryControlRequest::QueryRecords {
                query,
                scope,
                limit,
            } => {
                let scope = scope.map(scope_from_control);
                let records = self
                    .memory_store
                    .search(query, *limit)
                    .await?
                    .into_iter()
                    .map(|hit| hit.record)
                    .filter(|record| scope.as_ref().is_none_or(|scope| &record.scope == scope))
                    .take(*limit)
                    .map(record_event_view)
                    .collect();
                self.publish_memory_event(MemoryEvent::RecordsQueried { records });
            }
            MemoryControlRequest::QueryMetadata => {
                let labels = self
                    .memory_store
                    .list_labels(None)
                    .await?
                    .into_iter()
                    .map(|entry| MemoryLabelCountEvent {
                        label: label_to_string(entry.label).to_string(),
                        count: entry.count,
                    })
                    .collect();
                self.publish_memory_event(MemoryEvent::LabelsListed { labels });
            }
            MemoryControlRequest::SelectionSnapshot => {
                self.publish_memory_event(MemoryEvent::SelectionUpdated);
            }
        }
        Ok(())
    }

    fn publish_memory_event(&self, event: MemoryEvent) {
        let _ = self.event_bus.publish_control(RuntimeEvent::Memory(event));
    }
}

fn new_record_from_control(
    memory_id: &str,
    scope: ControlMemoryScope,
    content: &str,
    metadata: &serde_json::Value,
) -> Result<NewMemoryRecord> {
    Ok(NewMemoryRecord {
        id: Some(memory_id.to_string()),
        title: metadata
            .get("title")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .map(str::to_string),
        content: content.to_string(),
        labels: labels_from_metadata(metadata)?,
        importance: metadata
            .get("importance")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.5) as f32,
        pinned: metadata
            .get("pinned")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        source: MemorySource::ProtocolWrite,
        scope: scope_from_control(scope),
        session_id: metadata
            .get("session_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        thread_id: metadata
            .get("thread_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        source_span: None,
    })
}

fn patch_from_control(patch: &MemoryRecordControlPatch) -> Result<MemoryRecordPatch> {
    Ok(MemoryRecordPatch {
        title: patch.title.clone(),
        content: patch.content.clone(),
        labels: patch
            .labels
            .as_ref()
            .map(|labels| parse_labels(labels))
            .transpose()?,
        importance: patch.importance.map(|importance| importance as f32),
        pinned: patch.pinned,
        scope: patch.scope.map(scope_from_control),
        session_id: patch.session_id.clone(),
        thread_id: patch.thread_id.clone(),
        source_span: None,
    })
}

fn labels_from_metadata(metadata: &serde_json::Value) -> Result<Vec<MemoryLabel>> {
    let Some(labels) = metadata.get("labels") else {
        return Ok(vec![MemoryLabel::Experience]);
    };
    let labels = labels
        .as_array()
        .context("memory metadata labels must be an array")?
        .iter()
        .map(|label| {
            label
                .as_str()
                .context("memory metadata labels must be strings")
                .and_then(parse_label)
        })
        .collect::<Result<Vec<_>>>()?;
    if labels.is_empty() {
        Ok(vec![MemoryLabel::Experience])
    } else {
        Ok(labels)
    }
}

fn parse_labels(labels: &[String]) -> Result<Vec<MemoryLabel>> {
    labels.iter().map(|label| parse_label(label)).collect()
}

fn parse_label(label: &str) -> Result<MemoryLabel> {
    match label {
        "insight" => Ok(MemoryLabel::Insight),
        "decision" => Ok(MemoryLabel::Decision),
        "fact" => Ok(MemoryLabel::Fact),
        "procedure" => Ok(MemoryLabel::Procedure),
        "experience" => Ok(MemoryLabel::Experience),
        other => anyhow::bail!("unsupported memory label `{other}`"),
    }
}

fn label_to_string(label: MemoryLabel) -> &'static str {
    match label {
        MemoryLabel::Insight => "insight",
        MemoryLabel::Decision => "decision",
        MemoryLabel::Fact => "fact",
        MemoryLabel::Procedure => "procedure",
        MemoryLabel::Experience => "experience",
    }
}

fn record_event_view(record: MemoryRecord) -> MemoryRecordEventView {
    MemoryRecordEventView {
        id: record.id,
        title: record.title,
        content: record.content,
        labels: record
            .labels
            .into_iter()
            .map(label_to_string)
            .map(str::to_string)
            .collect(),
        importance_basis_points: (record.importance.clamp(0.0, 1.0) * 10_000.0).round() as u32,
        pinned: record.pinned,
        scope: scope_to_string(record.scope).to_string(),
        session_id: record.session_id,
        thread_id: record.thread_id,
    }
}

fn scope_to_string(scope: StoreMemoryScope) -> &'static str {
    match scope {
        StoreMemoryScope::User => "user",
        StoreMemoryScope::Workspace => "workspace",
        StoreMemoryScope::Project => "project",
        StoreMemoryScope::Thread => "thread",
        StoreMemoryScope::Session => "session",
    }
}

fn scope_from_control(scope: ControlMemoryScope) -> StoreMemoryScope {
    match scope {
        ControlMemoryScope::Thread => StoreMemoryScope::Thread,
        ControlMemoryScope::Workspace => StoreMemoryScope::Workspace,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use anyhow::Result;
    use serde_json::json;

    use super::*;
    use crate::llm::MockLlm;
    use crate::runtime_control::MemoryScope;
    use crate::vectordb::VectorDB;

    #[tokio::test]
    async fn memory_control_handler_mutates_memory_store_and_emits_events() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let bus = Arc::new(RuntimeEventBus::new(16));
        let mut events = bus.subscribe_control();
        let store = Arc::new(MemoryStore::new_with_record_path(
            Arc::new(MockLlm),
            Arc::new(VectorDB::new(
                temp.path().join("lancedb").to_str().expect("utf8 path"),
            )),
            temp.path().join("records.json"),
        ));
        let handler = MemoryControlHandler::new(bus, store.clone());

        handler
            .handle_control(&MemoryControlRequest::AddRecord {
                memory_id: "client-memory-1".to_string(),
                scope: MemoryScope::Workspace,
                content: "Use protocol memory writes through MemoryStore.".to_string(),
                metadata: json!({
                    "title": "Protocol memory writes",
                    "labels": ["decision"],
                    "importance": 0.8,
                    "pinned": true,
                    "session_id": "session-1"
                }),
            })
            .await?;

        let added = events.try_recv().expect("record added event");
        let RuntimeEvent::Memory(MemoryEvent::RecordAdded { memory_id }) = added.event else {
            panic!("expected record added event");
        };
        let record = store.get(&memory_id).await?.expect("stored record");
        assert_eq!(record.id, "client-memory-1");
        assert_eq!(record.title, "Protocol memory writes");
        assert_eq!(record.source, MemorySource::ProtocolWrite);
        assert_eq!(record.scope, StoreMemoryScope::Workspace);
        assert_eq!(record.labels, vec![MemoryLabel::Decision]);
        assert!(record.pinned);
        assert_eq!(record.session_id.as_deref(), Some("session-1"));

        handler
            .handle_control(&MemoryControlRequest::UpdateRecord {
                memory_id: memory_id.clone(),
                patch: MemoryRecordControlPatch {
                    labels: Some(vec!["fact".to_string()]),
                    pinned: Some(false),
                    ..Default::default()
                },
            })
            .await?;
        let updated = events.try_recv().expect("record updated event");
        assert!(matches!(
            updated.event,
            RuntimeEvent::Memory(MemoryEvent::RecordUpdated { .. })
        ));
        let record = store.get(&memory_id).await?.expect("updated record");
        assert_eq!(record.labels, vec![MemoryLabel::Fact]);
        assert!(!record.pinned);

        handler
            .handle_control(&MemoryControlRequest::QueryRecords {
                query: "protocol memory".to_string(),
                scope: Some(MemoryScope::Workspace),
                limit: 4,
            })
            .await?;
        let queried = events.try_recv().expect("records queried event");
        let RuntimeEvent::Memory(MemoryEvent::RecordsQueried { records }) = queried.event else {
            panic!("expected records queried event");
        };
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, memory_id);
        assert_eq!(records[0].labels, vec!["fact"]);

        handler
            .handle_control(&MemoryControlRequest::ListLabels {
                scope: Some(MemoryScope::Workspace),
            })
            .await?;
        let listed = events.try_recv().expect("labels listed event");
        let RuntimeEvent::Memory(MemoryEvent::LabelsListed { labels }) = listed.event else {
            panic!("expected labels listed event");
        };
        assert_eq!(labels[0].label, "fact");
        assert_eq!(labels[0].count, 1);

        handler
            .handle_control(&MemoryControlRequest::DeleteRecord {
                memory_id: memory_id.clone(),
            })
            .await?;
        let deleted = events.try_recv().expect("record deleted event");
        assert!(matches!(
            deleted.event,
            RuntimeEvent::Memory(MemoryEvent::RecordDeleted { .. })
        ));
        assert!(store.get(&memory_id).await?.is_none());

        Ok(())
    }

    #[tokio::test]
    async fn memory_control_handler_rejects_unknown_labels() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let bus = Arc::new(RuntimeEventBus::new(16));
        let store = Arc::new(MemoryStore::new_with_record_path(
            Arc::new(MockLlm),
            Arc::new(VectorDB::new(
                temp.path().join("lancedb").to_str().expect("utf8 path"),
            )),
            temp.path().join("records.json"),
        ));
        let handler = MemoryControlHandler::new(bus, store);

        let error = handler
            .handle_control(&MemoryControlRequest::AddRecord {
                memory_id: "client-memory-1".to_string(),
                scope: MemoryScope::Workspace,
                content: "bad label".to_string(),
                metadata: json!({ "labels": ["unknown"] }),
            })
            .await
            .expect_err("unknown label should fail");

        assert!(error.to_string().contains("unsupported memory label"));
        Ok(())
    }
}
