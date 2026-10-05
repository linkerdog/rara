use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use rara_skills::{ProtocolSkillError, ProtocolSkillRegistration, SkillManager, SkillSummary};

use crate::runtime_control::{
    RuntimeEvent, RuntimeProvenance, SkillEvent, SkillSourceControlRequest,
};
use crate::runtime_event_bus::RuntimeEventBus;

#[derive(Clone, Copy, Debug, thiserror::Error)]
pub enum SkillSourceError {
    #[error("skill source request is invalid")]
    Invalid,
    #[error("skill source operation is unsupported")]
    Unsupported,
    #[error("skill source capacity exceeded")]
    Capacity,
    #[error("skill source registry is unavailable")]
    Unavailable,
}

impl From<ProtocolSkillError> for SkillSourceError {
    fn from(error: ProtocolSkillError) -> Self {
        match error {
            ProtocolSkillError::Invalid | ProtocolSkillError::NotFound => Self::Invalid,
            ProtocolSkillError::Capacity => Self::Capacity,
        }
    }
}

/// Session-owned protocol metadata backed by the same catalogue as the native tool.
pub struct SkillSourceRegistry {
    event_bus: Arc<RuntimeEventBus>,
    manager: Option<Arc<RwLock<SkillManager>>>,
    origins: RwLock<BTreeMap<(String, String), RuntimeProvenance>>,
}

impl SkillSourceRegistry {
    #[cfg(test)]
    pub fn new(event_bus: Arc<RuntimeEventBus>) -> Self {
        Self::with_manager(event_bus, Arc::new(RwLock::new(SkillManager::new())))
    }

    pub fn with_manager(
        event_bus: Arc<RuntimeEventBus>,
        manager: Arc<RwLock<SkillManager>>,
    ) -> Self {
        Self {
            event_bus,
            manager: Some(manager),
            origins: RwLock::new(BTreeMap::new()),
        }
    }

    pub fn unavailable(event_bus: Arc<RuntimeEventBus>) -> Self {
        Self {
            event_bus,
            manager: None,
            origins: RwLock::new(BTreeMap::new()),
        }
    }

    pub async fn handle_control_with_provenance(
        &self,
        request: &SkillSourceControlRequest,
        provenance: RuntimeProvenance,
    ) -> Result<(), SkillSourceError> {
        for label in [
            &provenance.adapter,
            &provenance.session_id,
            &provenance.source_id,
        ]
        .into_iter()
        .flatten()
        {
            if label.is_empty()
                || label.len() > 128
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte))
            {
                return Err(SkillSourceError::Invalid);
            }
        }
        let manager = self.manager.as_ref().ok_or(SkillSourceError::Unsupported)?;
        // Always acquire origin metadata before the catalogue. Tool invocation
        // releases its catalogue read lock before it records source provenance.
        let mut origins = self
            .origins
            .write()
            .map_err(|_| SkillSourceError::Unavailable)?;
        let mut manager = manager.write().map_err(|_| SkillSourceError::Unavailable)?;
        match request {
            SkillSourceControlRequest::RegisterRoot { .. } => {
                return Err(SkillSourceError::Unsupported);
            }
            SkillSourceControlRequest::RegisterSkill {
                source_id,
                name,
                content,
                precedence_hint,
            } => {
                manager.register_protocol_skill(ProtocolSkillRegistration {
                    source_id: source_id.clone(),
                    name: name.clone(),
                    content: content.clone(),
                    precedence_hint: *precedence_hint,
                })?;
                let mut origin = provenance.clone();
                origin.source_id = Some(source_id.clone());
                origins.insert((source_id.clone(), name.clone()), origin.clone());
                self.event_bus.publish_control_with_turn(
                    RuntimeEvent::Skill(SkillEvent::Registered {
                        source_id: source_id.clone(),
                        name: name.clone(),
                    }),
                    origin,
                    None,
                );
            }
            SkillSourceControlRequest::DisableSkill { name, source_id } => {
                manager.disable_protocol_skill(name, source_id.as_deref())?;
                for ((source, candidate), origin) in origins.iter() {
                    if candidate == name
                        && source_id
                            .as_ref()
                            .is_none_or(|requested| source == requested)
                    {
                        self.event_bus.publish_control_with_turn(
                            RuntimeEvent::Skill(SkillEvent::Disabled {
                                source_id: source.clone(),
                                name: name.clone(),
                            }),
                            origin.clone(),
                            None,
                        );
                    }
                }
            }
            SkillSourceControlRequest::QuerySkills => {}
        }
        self.event_bus.publish_control_with_turn(
            RuntimeEvent::Skill(SkillEvent::Catalogue {
                skills: manager.protocol_skill_statuses(),
            }),
            provenance,
            None,
        );
        Ok(())
    }

    pub fn prompt_summaries(&self) -> Result<Option<Vec<SkillSummary>>, SkillSourceError> {
        self.manager
            .as_ref()
            .map(|manager| {
                manager
                    .read()
                    .map(|manager| manager.list_summaries())
                    .map_err(|_| SkillSourceError::Unavailable)
            })
            .transpose()
    }

    /// Called only after the native tool has selected and returned this source's body.
    pub fn record_invocation(
        &self,
        source_id: &str,
        name: &str,
        turn_id: Option<&str>,
    ) -> Result<(), SkillSourceError> {
        let origins = self
            .origins
            .read()
            .map_err(|_| SkillSourceError::Unavailable)?;
        let origin = origins
            .get(&(source_id.to_owned(), name.to_owned()))
            .ok_or(SkillSourceError::Unavailable)?;
        self.event_bus.publish_control_with_turn(
            RuntimeEvent::Skill(SkillEvent::Injected {
                source_id: source_id.to_owned(),
                name: name.to_owned(),
            }),
            origin.clone(),
            turn_id,
        );
        Ok(())
    }
}
