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

use tokio::sync::RwLock;

use crate::runtime_control::{
    MemoryControlRequest, PromptSourceControlRequest, PromptSourceEvent, PromptSourceLifetime,
    PromptSourceRegistration, RuntimeEvent, SkillSourceControlRequest,
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
/// Currently records the request and emits an event; full integration
/// with the memory backend (`MemoryBackend`) will add/update/delete
/// the actual store.
pub struct MemoryControlHandler {
    event_bus: Arc<RuntimeEventBus>,
}

impl MemoryControlHandler {
    pub fn new(event_bus: Arc<RuntimeEventBus>) -> Self {
        Self { event_bus }
    }

    #[allow(unused_variables)]
    pub async fn handle_control(&self, request: &MemoryControlRequest) {
        // Memory integration is pending the MemoryBackend trait refactor.
        // For now, record the request through an event.
        let memory_id = match request {
            MemoryControlRequest::AddRecord { memory_id, .. } => memory_id.clone(),
            MemoryControlRequest::UpdateRecord { memory_id, .. } => memory_id.clone(),
            MemoryControlRequest::DeleteRecord { memory_id } => memory_id.clone(),
            MemoryControlRequest::ListLabels { .. } => "query".into(),
            MemoryControlRequest::QueryMetadata => "query".into(),
            MemoryControlRequest::SelectionSnapshot => "snapshot".into(),
        };
        let _ = self.event_bus.publish_control(RuntimeEvent::Memory(
            crate::runtime_control::MemoryEvent::RecordAdded { memory_id },
        ));
    }
}
