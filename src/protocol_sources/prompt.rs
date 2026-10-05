use std::collections::BTreeMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::prompt::{PromptSource, PromptSourceKind};
use crate::runtime_control::{
    PromptSourceControlRequest, PromptSourceEvent, PromptSourceLifetime, PromptSourceRegistration,
    RuntimeEvent, RuntimeProvenance, SourceLayer, SourceScope,
};
use crate::runtime_event_bus::RuntimeEventBus;

// ── Prompt source registry ──────────────────────────────────────────────

/// Stored entry for a protocol-registered prompt source.
#[derive(Clone, Debug)]
struct PromptSourceEntry {
    registration: PromptSourceRegistration,
    provenance: RuntimeProvenance,
    /// Remaining turn count (only meaningful for `Turns` lifetime).
    remaining_turns: Option<u32>,
}

/// Stable snapshot of a protocol-registered prompt source.
///
/// Keeps provenance and the current query-count lifetime alongside source content
/// for explainable prompt runtime and `/context` integration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolPromptSourceSnapshot {
    pub registration: PromptSourceRegistration,
    pub provenance: RuntimeProvenance,
    pub remaining_turns: Option<u32>,
}

impl From<&PromptSourceEntry> for ProtocolPromptSourceSnapshot {
    fn from(entry: &PromptSourceEntry) -> Self {
        Self {
            registration: entry.registration.clone(),
            provenance: entry.provenance.clone(),
            remaining_turns: entry.remaining_turns,
        }
    }
}

impl ProtocolPromptSourceSnapshot {
    pub fn to_prompt_source(&self) -> PromptSource {
        PromptSource {
            kind: PromptSourceKind::ProtocolPromptSource,
            label: format!("Protocol Prompt Source {}", self.registration.source_id),
            display_path: self.display_path(),
            content: self.registration.content.clone(),
        }
    }

    fn display_path(&self) -> String {
        let controller = format!("{:?}", self.provenance.controller).to_lowercase();
        match self.provenance.adapter.as_deref() {
            Some(adapter) if !adapter.trim().is_empty() => {
                format!(
                    "protocol:{controller}:{adapter}:{}",
                    self.registration.source_id
                )
            }
            _ => format!("protocol:{controller}:{}", self.registration.source_id),
        }
    }
}

/// Registry for protocol-registered prompt sources.
pub struct PromptSourceRegistry {
    event_bus: Arc<RuntimeEventBus>,
    sources: RwLock<BTreeMap<String, PromptSourceEntry>>,
}

pub(crate) const MAX_PROMPT_SOURCES: usize = 32;
pub(crate) const MAX_PROMPT_SOURCE_BYTES: usize = 64 * 1024;
pub(crate) const MAX_PROMPT_CONTENT_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PromptSourceError {
    #[error("prompt source registration is invalid")]
    Invalid,
    #[error("prompt source scope, layer, or lifetime is unsupported")]
    Unsupported,
    #[error("prompt source capacity exceeded")]
    Capacity,
}

fn valid_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte))
}

impl PromptSourceRegistry {
    pub fn new(event_bus: Arc<RuntimeEventBus>) -> Self {
        Self {
            event_bus,
            sources: RwLock::new(BTreeMap::new()),
        }
    }

    /// Apply bounded session context without claiming unsupported storage or authority.
    pub async fn handle_control_with_provenance(
        &self,
        request: &PromptSourceControlRequest,
        provenance: RuntimeProvenance,
    ) -> Result<(), PromptSourceError> {
        for label in [
            &provenance.adapter,
            &provenance.session_id,
            &provenance.source_id,
        ]
        .into_iter()
        .flatten()
        {
            if !valid_identity(label) {
                return Err(PromptSourceError::Invalid);
            }
        }
        match request {
            PromptSourceControlRequest::Register(registration) => {
                if !valid_identity(&registration.source_id)
                    || registration.content.trim().is_empty()
                    || matches!(registration.lifetime, PromptSourceLifetime::Turns(0))
                {
                    return Err(PromptSourceError::Invalid);
                }
                if !matches!(
                    registration.scope,
                    SourceScope::Session | SourceScope::Protocol
                ) || !matches!(registration.layer, SourceLayer::User)
                    || matches!(registration.lifetime, PromptSourceLifetime::Persistent)
                {
                    return Err(PromptSourceError::Unsupported);
                }
                if registration.content.len() > MAX_PROMPT_SOURCE_BYTES {
                    return Err(PromptSourceError::Capacity);
                }
                let mut sources = self.sources.write().await;
                let replaces = sources.contains_key(&registration.source_id);
                let retained_bytes: usize = sources
                    .iter()
                    .filter(|(id, _)| *id != &registration.source_id)
                    .map(|(_, entry)| entry.registration.content.len())
                    .sum();
                if (!replaces && sources.len() >= MAX_PROMPT_SOURCES)
                    || retained_bytes.saturating_add(registration.content.len())
                        > MAX_PROMPT_CONTENT_BYTES
                {
                    return Err(PromptSourceError::Capacity);
                }
                let remaining_turns = match registration.lifetime {
                    PromptSourceLifetime::Turns(turns) => Some(turns),
                    PromptSourceLifetime::Session => None,
                    PromptSourceLifetime::Persistent => return Err(PromptSourceError::Unsupported),
                };
                sources.insert(
                    registration.source_id.clone(),
                    PromptSourceEntry {
                        registration: registration.clone(),
                        provenance: provenance.clone(),
                        remaining_turns,
                    },
                );
                self.publish(
                    PromptSourceEvent::Registered {
                        source_id: registration.source_id.clone(),
                    },
                    provenance,
                );
            }
            PromptSourceControlRequest::Unregister { source_id } => {
                if !valid_identity(source_id) {
                    return Err(PromptSourceError::Invalid);
                }
                let mut sources = self.sources.write().await;
                if let Some(entry) = sources.remove(source_id) {
                    self.publish(
                        PromptSourceEvent::Unregistered {
                            source_id: source_id.clone(),
                        },
                        entry.provenance,
                    );
                }
            }
            PromptSourceControlRequest::QuerySources => {
                let sources = self.sources.read().await;
                for (id, entry) in sources.iter() {
                    self.publish(
                        PromptSourceEvent::Registered {
                            source_id: id.clone(),
                        },
                        entry.provenance.clone(),
                    );
                }
            }
        }
        Ok(())
    }

    /// Snapshot active sources and advance query-count lifetimes under one lock.
    pub async fn list_prompt_sources_for_query(&self) -> Vec<PromptSource> {
        let mut sources = self.sources.write().await;
        let mut prompt_sources = Vec::with_capacity(sources.len());
        let mut expired = Vec::new();
        for (id, entry) in sources.iter_mut() {
            prompt_sources.push(ProtocolPromptSourceSnapshot::from(&*entry).to_prompt_source());
            self.publish(
                PromptSourceEvent::Injected {
                    source_id: id.clone(),
                },
                entry.provenance.clone(),
            );
            if let Some(ref mut remaining) = entry.remaining_turns {
                if *remaining <= 1 {
                    expired.push(id.clone());
                } else {
                    *remaining -= 1;
                }
            }
        }
        for id in expired {
            if let Some(entry) = sources.remove(&id) {
                self.publish(
                    PromptSourceEvent::Dropped {
                        source_id: id,
                        reason: "turn limit expired".into(),
                    },
                    entry.provenance,
                );
            }
        }
        prompt_sources
    }

    fn publish(&self, event: PromptSourceEvent, provenance: RuntimeProvenance) {
        self.event_bus.publish_control_with_turn(
            RuntimeEvent::PromptSource(event),
            provenance,
            None,
        );
    }
}

#[cfg(test)]
#[path = "prompt_tests.rs"]
mod tests;
