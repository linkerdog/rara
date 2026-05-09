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

use anyhow::{Result, bail};
use serde_json::Value;
use tokio::sync::RwLock;

use crate::memory_store::{
    MemoryLabel, MemoryLabelCount, MemoryPromotionTarget, MemoryRecord, MemoryRecordPatch,
    MemoryScope as StoreMemoryScope, MemorySource, MemoryStore, NewMemoryRecord,
};
use crate::runtime_control::{
    MemoryControlRequest, MemoryEvent, MemoryLabelSummary, MemoryRecordControlPatch,
    MemoryRecordSummary, MemoryScope as ControlMemoryScope, PromptSourceControlRequest,
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
///
pub struct MemoryControlHandler {
    event_bus: Arc<RuntimeEventBus>,
    memory_store: Option<Arc<MemoryStore>>,
}

impl MemoryControlHandler {
    pub fn new(event_bus: Arc<RuntimeEventBus>) -> Self {
        Self {
            event_bus,
            memory_store: None,
        }
    }

    pub fn with_store(event_bus: Arc<RuntimeEventBus>, memory_store: Arc<MemoryStore>) -> Self {
        Self {
            event_bus,
            memory_store: Some(memory_store),
        }
    }

    pub async fn handle_control(&self, request: &MemoryControlRequest) -> Result<()> {
        let Some(memory_store) = &self.memory_store else {
            self.publish_scaffold_event(request);
            return Ok(());
        };

        match request {
            MemoryControlRequest::AddRecord {
                memory_id,
                scope,
                content,
                metadata,
            } => {
                if memory_store.get(memory_id).await?.is_some() {
                    bail!("memory record {memory_id:?} already exists");
                }
                let record = memory_store
                    .insert_with_id(
                        Some(memory_id.clone()),
                        new_memory_record_from_control(scope, content, metadata)?,
                    )
                    .await?;
                self.publish_memory_event(MemoryEvent::RecordAdded {
                    memory_id: record.id,
                });
            }
            MemoryControlRequest::UpdateRecord { memory_id, patch } => {
                memory_store
                    .update(memory_id, memory_patch_from_control(patch)?)
                    .await?;
                self.publish_memory_event(MemoryEvent::RecordUpdated {
                    memory_id: memory_id.clone(),
                });
            }
            MemoryControlRequest::DeleteRecord { memory_id } => {
                memory_store.delete(memory_id).await?;
                self.publish_memory_event(MemoryEvent::RecordDeleted {
                    memory_id: memory_id.clone(),
                });
            }
            MemoryControlRequest::ListLabels { scope } => {
                let labels = memory_store
                    .list_labels(scope.clone().map(store_scope_from_control))
                    .await?;
                self.publish_memory_event(MemoryEvent::LabelsListed {
                    scope: scope.clone(),
                    labels: label_summaries(labels),
                });
            }
            MemoryControlRequest::QueryRecords {
                query,
                scope,
                limit,
            } => {
                let store_scope = scope.clone().map(store_scope_from_control);
                let records = memory_store
                    .search(query, *limit)
                    .await?
                    .into_iter()
                    .map(|hit| hit.record)
                    .filter(|record| {
                        store_scope
                            .as_ref()
                            .is_none_or(|scope| &record.scope == scope)
                    })
                    .take(*limit)
                    .map(memory_record_summary)
                    .collect();
                self.publish_memory_event(MemoryEvent::RecordsQueried { records });
            }
            MemoryControlRequest::QueryMetadata => {
                let labels = memory_store.list_labels(None).await?;
                let record_count = memory_store.record_count().await?;
                self.publish_memory_event(MemoryEvent::MetadataQueried {
                    record_count,
                    labels: label_summaries(labels),
                });
            }
            MemoryControlRequest::SelectionSnapshot => {
                self.publish_memory_event(MemoryEvent::SelectionUpdated);
            }
        }
        Ok(())
    }

    fn publish_scaffold_event(&self, request: &MemoryControlRequest) {
        let event = match request {
            MemoryControlRequest::AddRecord { memory_id, .. } => MemoryEvent::RecordAdded {
                memory_id: memory_id.clone(),
            },
            MemoryControlRequest::UpdateRecord { memory_id, .. } => MemoryEvent::RecordUpdated {
                memory_id: memory_id.clone(),
            },
            MemoryControlRequest::DeleteRecord { memory_id } => MemoryEvent::RecordDeleted {
                memory_id: memory_id.clone(),
            },
            MemoryControlRequest::ListLabels { scope } => MemoryEvent::LabelsListed {
                scope: scope.clone(),
                labels: Vec::new(),
            },
            MemoryControlRequest::QueryRecords { .. } => MemoryEvent::RecordsQueried {
                records: Vec::new(),
            },
            MemoryControlRequest::QueryMetadata => MemoryEvent::MetadataQueried {
                record_count: 0,
                labels: Vec::new(),
            },
            MemoryControlRequest::SelectionSnapshot => MemoryEvent::SelectionUpdated,
        };
        self.publish_memory_event(event);
    }

    fn publish_memory_event(&self, event: MemoryEvent) {
        let _ = self.event_bus.publish_control(RuntimeEvent::Memory(event));
    }
}

fn new_memory_record_from_control(
    scope: &ControlMemoryScope,
    content: &str,
    metadata: &Value,
) -> Result<NewMemoryRecord> {
    let target = match scope {
        ControlMemoryScope::Workspace => MemoryPromotionTarget::Workspace {
            session_id: metadata_string(metadata, "session_id"),
            thread_id: metadata_string(metadata, "thread_id"),
        },
        ControlMemoryScope::Thread => MemoryPromotionTarget::Thread {
            session_id: metadata_string(metadata, "session_id"),
            thread_id: metadata_string(metadata, "thread_id")
                .ok_or_else(|| anyhow::anyhow!("thread_id is required for thread memory scope"))?,
            source_span: None,
        },
    };
    let mut record = NewMemoryRecord::promotion_base(MemorySource::ProtocolWrite, target)?;
    record.title = metadata_string(metadata, "title");
    record.content = content.to_string();
    record.labels = metadata_labels(metadata)?;
    record.importance = metadata
        .get("importance")
        .and_then(Value::as_f64)
        .unwrap_or(0.5) as f32;
    record.pinned = metadata
        .get("pinned")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(record)
}

fn memory_patch_from_control(patch: &MemoryRecordControlPatch) -> Result<MemoryRecordPatch> {
    Ok(MemoryRecordPatch {
        title: patch.title.clone(),
        content: patch.content.clone(),
        labels: patch
            .labels
            .as_ref()
            .map(|labels| {
                labels
                    .iter()
                    .map(|label| memory_label_from_str(label))
                    .collect()
            })
            .transpose()?,
        importance: patch.importance.map(|importance| importance as f32),
        pinned: patch.pinned,
        scope: patch.scope.clone().map(store_scope_from_control),
        session_id: patch.session_id.clone(),
        thread_id: patch.thread_id.clone(),
        source_span: None,
    })
}

fn metadata_labels(metadata: &Value) -> Result<Vec<MemoryLabel>> {
    let Some(labels) = metadata.get("labels") else {
        return Ok(Vec::new());
    };
    let Some(labels) = labels.as_array() else {
        bail!("memory metadata labels must be an array");
    };
    labels
        .iter()
        .map(|label| {
            let Some(label) = label.as_str() else {
                bail!("memory metadata labels must be strings");
            };
            memory_label_from_str(label)
        })
        .collect()
}

fn metadata_string(metadata: &Value, key: &str) -> Option<String> {
    metadata
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn memory_label_from_str(label: &str) -> Result<MemoryLabel> {
    match label.trim().to_ascii_lowercase().as_str() {
        "insight" => Ok(MemoryLabel::Insight),
        "decision" => Ok(MemoryLabel::Decision),
        "fact" => Ok(MemoryLabel::Fact),
        "procedure" => Ok(MemoryLabel::Procedure),
        "experience" => Ok(MemoryLabel::Experience),
        other => bail!("unknown memory label {other:?}"),
    }
}

fn store_scope_from_control(scope: ControlMemoryScope) -> StoreMemoryScope {
    match scope {
        ControlMemoryScope::Thread => StoreMemoryScope::Thread,
        ControlMemoryScope::Workspace => StoreMemoryScope::Workspace,
    }
}

fn label_summaries(labels: Vec<MemoryLabelCount>) -> Vec<MemoryLabelSummary> {
    labels
        .into_iter()
        .map(|label| MemoryLabelSummary {
            label: memory_label_name(&label.label).to_string(),
            count: label.count,
        })
        .collect()
}

fn memory_record_summary(record: MemoryRecord) -> MemoryRecordSummary {
    MemoryRecordSummary {
        id: record.id,
        title: record.title,
        content: record.content,
        labels: record
            .labels
            .into_iter()
            .map(|label| memory_label_name(&label).to_string())
            .collect(),
        importance_basis_points: (record.importance.clamp(0.0, 1.0) * 10_000.0).round() as u32,
        pinned: record.pinned,
        scope: memory_scope_name(record.scope).to_string(),
        session_id: record.session_id,
        thread_id: record.thread_id,
    }
}

fn memory_scope_name(scope: StoreMemoryScope) -> &'static str {
    match scope {
        StoreMemoryScope::User => "user",
        StoreMemoryScope::Workspace => "workspace",
        StoreMemoryScope::Project => "project",
        StoreMemoryScope::Thread => "thread",
        StoreMemoryScope::Session => "session",
    }
}

fn memory_label_name(label: &MemoryLabel) -> &'static str {
    match label {
        MemoryLabel::Insight => "insight",
        MemoryLabel::Decision => "decision",
        MemoryLabel::Fact => "fact",
        MemoryLabel::Procedure => "procedure",
        MemoryLabel::Experience => "experience",
    }
}

#[cfg(test)]
mod tests {
    use rara_memory::vectordb::VectorDB;
    use serde_json::json;

    use super::*;
    use crate::llm::MockLlm;
    use crate::runtime_control::RuntimeEvent;

    fn test_memory_store(root: &std::path::Path) -> Arc<MemoryStore> {
        Arc::new(MemoryStore::new_with_record_path(
            Arc::new(MockLlm),
            Arc::new(VectorDB::new(
                root.join("lancedb").to_str().expect("utf8 path"),
            )),
            root.join("memories").join("records.json"),
        ))
    }

    #[tokio::test]
    async fn memory_control_add_record_writes_memory_store_and_emits_actual_id() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bus = Arc::new(RuntimeEventBus::new(8));
        let mut events = bus.subscribe_control();
        let store = test_memory_store(temp.path());
        let handler = MemoryControlHandler::with_store(bus, store.clone());

        handler
            .handle_control(&MemoryControlRequest::AddRecord {
                memory_id: "protocol-memory-1".to_string(),
                scope: ControlMemoryScope::Workspace,
                content: "ACP memory writes should go through MemoryStore.".to_string(),
                metadata: json!({
                    "title": "ACP memory bridge",
                    "labels": ["decision", "fact"],
                    "importance": 0.9,
                    "pinned": true,
                    "thread_id": "thread-1"
                }),
            })
            .await
            .expect("add memory");

        let saved = store
            .get("protocol-memory-1")
            .await
            .expect("get memory")
            .expect("saved memory");
        assert_eq!(saved.id, "protocol-memory-1");
        assert_eq!(saved.title, "ACP memory bridge");
        assert_eq!(saved.labels, vec![MemoryLabel::Decision, MemoryLabel::Fact]);
        assert_eq!(saved.source, MemorySource::ProtocolWrite);
        assert_eq!(saved.scope, StoreMemoryScope::Workspace);
        assert_eq!(saved.thread_id.as_deref(), Some("thread-1"));
        assert!(saved.pinned);

        let event = events.recv().await.expect("memory event");
        assert!(matches!(
            event.event,
            RuntimeEvent::Memory(MemoryEvent::RecordAdded { memory_id })
                if memory_id == "protocol-memory-1"
        ));
    }

    #[tokio::test]
    async fn memory_control_update_delete_and_labels_use_memory_store() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bus = Arc::new(RuntimeEventBus::new(16));
        let mut events = bus.subscribe_control();
        let store = test_memory_store(temp.path());
        let handler = MemoryControlHandler::with_store(bus, store.clone());
        store
            .insert_with_id(
                Some("protocol-memory-2".to_string()),
                NewMemoryRecord {
                    title: Some("Initial".to_string()),
                    content: "Initial memory body.".to_string(),
                    labels: vec![MemoryLabel::Experience],
                    importance: 0.5,
                    pinned: false,
                    source: MemorySource::ProtocolWrite,
                    scope: StoreMemoryScope::Thread,
                    session_id: None,
                    thread_id: Some("thread-2".to_string()),
                    source_span: None,
                },
            )
            .await
            .expect("seed memory");

        handler
            .handle_control(&MemoryControlRequest::UpdateRecord {
                memory_id: "protocol-memory-2".to_string(),
                patch: MemoryRecordControlPatch {
                    title: Some("Updated".to_string()),
                    labels: Some(vec!["procedure".to_string()]),
                    importance: Some(0.8),
                    scope: Some(ControlMemoryScope::Workspace),
                    ..Default::default()
                },
            })
            .await
            .expect("update memory");

        let updated = store
            .get("protocol-memory-2")
            .await
            .expect("get memory")
            .expect("updated memory");
        assert_eq!(updated.title, "Updated");
        assert_eq!(updated.labels, vec![MemoryLabel::Procedure]);
        assert_eq!(updated.scope, StoreMemoryScope::Workspace);

        handler
            .handle_control(&MemoryControlRequest::ListLabels { scope: None })
            .await
            .expect("list labels");

        let mut saw_update = false;
        let mut saw_labels = false;
        for _ in 0..2 {
            let event = events.recv().await.expect("memory event");
            match event.event {
                RuntimeEvent::Memory(MemoryEvent::RecordUpdated { memory_id }) => {
                    saw_update = memory_id == "protocol-memory-2";
                }
                RuntimeEvent::Memory(MemoryEvent::LabelsListed { labels, .. }) => {
                    saw_labels = labels
                        == vec![MemoryLabelSummary {
                            label: "procedure".to_string(),
                            count: 1,
                        }];
                }
                _ => {}
            }
        }
        assert!(saw_update);
        assert!(saw_labels);

        handler
            .handle_control(&MemoryControlRequest::QueryRecords {
                query: "Initial memory".to_string(),
                scope: Some(ControlMemoryScope::Workspace),
                limit: 4,
            })
            .await
            .expect("query records");

        let event = events.recv().await.expect("records queried event");
        let RuntimeEvent::Memory(MemoryEvent::RecordsQueried { records }) = event.event else {
            panic!("expected records queried event");
        };
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, "protocol-memory-2");
        assert_eq!(records[0].labels, vec!["procedure"]);
        assert_eq!(records[0].scope, "workspace");

        handler
            .handle_control(&MemoryControlRequest::DeleteRecord {
                memory_id: "protocol-memory-2".to_string(),
            })
            .await
            .expect("delete memory");

        assert!(
            store
                .get("protocol-memory-2")
                .await
                .expect("get deleted memory")
                .is_none()
        );
    }

    #[tokio::test]
    async fn memory_control_without_store_keeps_scaffold_events() {
        let bus = Arc::new(RuntimeEventBus::new(8));
        let mut events = bus.subscribe_control();
        let handler = MemoryControlHandler::new(bus);

        handler
            .handle_control(&MemoryControlRequest::ListLabels {
                scope: Some(ControlMemoryScope::Thread),
            })
            .await
            .expect("list labels");

        let event = events.recv().await.expect("memory event");
        assert!(matches!(
            event.event,
            RuntimeEvent::Memory(MemoryEvent::LabelsListed { scope: Some(ControlMemoryScope::Thread), labels })
                if labels.is_empty()
        ));
    }

    #[tokio::test]
    async fn memory_control_add_record_rejects_duplicate_protocol_id() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bus = Arc::new(RuntimeEventBus::new(8));
        let store = test_memory_store(temp.path());
        let handler = MemoryControlHandler::with_store(bus, store.clone());
        let request = MemoryControlRequest::AddRecord {
            memory_id: "protocol-memory-duplicate".to_string(),
            scope: ControlMemoryScope::Thread,
            content: "Duplicate protocol ids should not upsert.".to_string(),
            metadata: json!({"labels": ["fact"], "thread_id": "thread-duplicate"}),
        };

        handler
            .handle_control(&request)
            .await
            .expect("first add memory");
        let err = handler
            .handle_control(&request)
            .await
            .expect_err("duplicate add should fail");

        assert!(err.to_string().contains("already exists"));
    }

    #[tokio::test]
    async fn memory_control_thread_record_requires_thread_id() {
        let temp = tempfile::tempdir().expect("tempdir");
        let bus = Arc::new(RuntimeEventBus::new(8));
        let store = test_memory_store(temp.path());
        let handler = MemoryControlHandler::with_store(bus, store);

        let err = handler
            .handle_control(&MemoryControlRequest::AddRecord {
                memory_id: "protocol-memory-thread-missing-id".to_string(),
                scope: ControlMemoryScope::Thread,
                content: "Thread-scoped protocol writes need an explicit thread id.".to_string(),
                metadata: json!({"labels": ["fact"]}),
            })
            .await
            .expect_err("thread scope without thread_id should fail");

        assert!(err.to_string().contains("thread_id is required"));
    }
}
