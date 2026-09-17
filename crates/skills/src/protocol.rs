use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use serde::Serialize;

use crate::{Skill, SkillManager, SkillScope, strip_frontmatter};

pub const MAX_PROTOCOL_SKILLS: usize = 32;
pub const MAX_PROTOCOL_SKILL_BYTES: usize = 64 * 1024;
pub const MAX_PROTOCOL_TOTAL_BYTES: usize = 256 * 1024;
const MAX_ID_BYTES: usize = 128;

/// One session-owned inline definition; precedence never outranks local discovery.
#[derive(Clone, Debug)]
pub struct ProtocolSkillRegistration {
    pub source_id: String,
    pub name: String,
    pub content: String,
    pub precedence_hint: Option<i32>,
}

/// Body-free status for protocol queries, including retained disabled/shadowed entries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProtocolSkillStatus {
    pub source_id: String,
    pub name: String,
    pub priority: i32,
    pub registration_order: u64,
    pub enabled: bool,
    pub selected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolSkillError {
    Invalid,
    Capacity,
    NotFound,
}

impl fmt::Display for ProtocolSkillError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Invalid => "protocol skill definition is invalid",
            Self::Capacity => "protocol skill capacity exceeded",
            Self::NotFound => "protocol skill definition does not exist",
        })
    }
}

impl std::error::Error for ProtocolSkillError {}

pub(super) struct ProtocolEntry {
    pub(super) source_id: String,
    pub(super) skill: Skill,
    priority: i32,
    order: u64,
    enabled: bool,
}

#[derive(Default)]
pub(super) struct ProtocolCatalogue {
    entries: BTreeMap<(String, String), ProtocolEntry>,
    next_order: u64,
}

impl SkillManager {
    /// Validate and atomically register or replace a source/name pair.
    pub fn register_protocol_skill(
        &mut self,
        registration: ProtocolSkillRegistration,
    ) -> Result<(), ProtocolSkillError> {
        if !valid_id(&registration.source_id)
            || !valid_id(&registration.name)
            || registration.content.trim().is_empty()
        {
            return Err(ProtocolSkillError::Invalid);
        }
        if registration.content.len() > MAX_PROTOCOL_SKILL_BYTES {
            return Err(ProtocolSkillError::Capacity);
        }
        let key = (registration.source_id.clone(), registration.name.clone());
        let existing = self.protocol.entries.get(&key);
        if existing.is_none() && self.protocol.entries.len() >= MAX_PROTOCOL_SKILLS {
            return Err(ProtocolSkillError::Capacity);
        }
        let retained_bytes: usize = self
            .protocol
            .entries
            .iter()
            .filter(|(entry_key, _)| **entry_key != key)
            .map(|(_, entry)| entry.skill.content.len())
            .sum();
        if retained_bytes + registration.content.len() > MAX_PROTOCOL_TOTAL_BYTES {
            return Err(ProtocolSkillError::Capacity);
        }
        let order = existing.map_or(self.protocol.next_order, |entry| entry.order);
        let next_order = if existing.is_some() {
            self.protocol.next_order
        } else {
            self.protocol
                .next_order
                .checked_add(1)
                .ok_or(ProtocolSkillError::Capacity)?
        };
        let body = strip_frontmatter(&registration.content);
        if body.trim().is_empty() {
            return Err(ProtocolSkillError::Invalid);
        }
        // Follow the native Markdown fallback; do not interpret unowned YAML policy.
        let description = body
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty() && !line.starts_with('#'))
            .unwrap_or("No description provided.")
            .chars()
            .take(512)
            .collect();
        let skill = Skill {
            path: PathBuf::from(format!(
                "protocol:{}/{}",
                registration.source_id, registration.name
            )),
            title: Some(registration.name.clone()),
            name: registration.name,
            description,
            scope: SkillScope::Protocol,
            content: registration.content,
            disable_model_invocation: false,
        };
        self.protocol.entries.insert(
            key,
            ProtocolEntry {
                source_id: registration.source_id,
                skill,
                priority: registration.precedence_hint.unwrap_or(0),
                order,
                enabled: true,
            },
        );
        self.protocol.next_order = next_order;
        Ok(())
    }

    /// Disable matching protocol definitions without mutating any local skill.
    pub fn disable_protocol_skill(
        &mut self,
        name: &str,
        source_id: Option<&str>,
    ) -> Result<(), ProtocolSkillError> {
        if !valid_id(name) || source_id.is_some_and(|source| !valid_id(source)) {
            return Err(ProtocolSkillError::Invalid);
        }
        let mut found = false;
        for ((source, candidate), entry) in &mut self.protocol.entries {
            if candidate == name && source_id.is_none_or(|requested| source == requested) {
                found = true;
                entry.enabled = false;
            }
        }
        if found {
            Ok(())
        } else {
            Err(ProtocolSkillError::NotFound)
        }
    }

    pub fn protocol_skill_statuses(&self) -> Vec<ProtocolSkillStatus> {
        self.protocol
            .entries
            .values()
            .map(|entry| ProtocolSkillStatus {
                source_id: entry.source_id.clone(),
                name: entry.skill.name.clone(),
                priority: entry.priority,
                registration_order: entry.order,
                enabled: entry.enabled,
                selected: self.winning_protocol_source(&entry.skill.name)
                    == Some(entry.source_id.as_str()),
            })
            .collect()
    }

    /// Identify the winning protocol source, or None for a local/absent skill.
    pub fn winning_protocol_source(&self, name: &str) -> Option<&str> {
        if self.skills.contains_key(name) {
            return None;
        }
        self.protocol_winner(name)
            .map(|entry| entry.source_id.as_str())
    }

    pub(super) fn protocol_names(&self) -> impl Iterator<Item = &str> {
        self.protocol
            .entries
            .values()
            .filter(|entry| entry.enabled)
            .map(|entry| entry.skill.name.as_str())
    }

    pub(super) fn protocol_candidates(&self, name: &str) -> Vec<&ProtocolEntry> {
        let mut entries: Vec<_> = self
            .protocol
            .entries
            .values()
            .filter(|entry| entry.enabled && entry.skill.name == name)
            .collect();
        entries.sort_by(|left, right| {
            right
                .priority
                .cmp(&left.priority)
                .then_with(|| left.order.cmp(&right.order))
                .then_with(|| left.source_id.cmp(&right.source_id))
        });
        entries
    }

    pub(super) fn protocol_winner(&self, name: &str) -> Option<&ProtocolEntry> {
        self.protocol_candidates(name).into_iter().next()
    }
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte))
}

#[cfg(test)]
mod tests;
