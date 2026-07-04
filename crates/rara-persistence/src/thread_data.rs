use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedTurnEntry {
    pub role: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedPlanStep {
    pub step_index: usize,
    pub status: String,
    pub step: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedInteraction {
    pub kind: String,
    pub status: String,
    pub title: String,
    pub summary: String,
    pub payload: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistedPlanLifecycle {
    pub phase: String,
    #[serde(default)]
    pub decision: Option<String>,
    #[serde(default)]
    pub feedback: Option<String>,
    #[serde(default)]
    pub plan_path: Option<String>,
    #[serde(default)]
    pub tool_use_id: Option<String>,
    #[serde(default)]
    pub plan_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedTurnSummary {
    pub ordinal: usize,
    pub event_count: usize,
    pub artifact_path: String,
    pub preview: String,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PersistedRuntimeRolloutItem {
    PlanState {
        explanation: Option<String>,
        steps: Vec<PersistedPlanStep>,
    },
    Interaction(PersistedInteraction),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PersistedStructuredRolloutEvent {
    Compaction {
        #[serde(default)]
        recorded_at: Option<i64>,
        event_index: usize,
        before_tokens: usize,
        after_tokens: usize,
        boundary_version: u32,
        #[serde(default)]
        replaced_start: Option<usize>,
        #[serde(default)]
        replaced_end: Option<usize>,
        #[serde(default)]
        metadata_owner: Option<String>,
        recent_files: Vec<String>,
        summary: String,
    },
    RuntimeState {
        #[serde(default)]
        recorded_at: Option<i64>,
        explanation: Option<String>,
        steps: Vec<PersistedPlanStep>,
        interactions: Vec<PersistedInteraction>,
        #[serde(default)]
        plan_lifecycle: Vec<PersistedPlanLifecycle>,
    },
    PlanState {
        #[serde(default)]
        recorded_at: Option<i64>,
        explanation: Option<String>,
        steps: Vec<PersistedPlanStep>,
    },
    Interaction {
        #[serde(default)]
        recorded_at: Option<i64>,
        #[serde(flatten)]
        interaction: PersistedInteraction,
    },
    SpawnAgent {
        #[serde(default)]
        recorded_at: Option<i64>,
        event_id: String,
        agent_id: String,
        name: Option<String>,
        child_session_id: String,
        status: String,
        summary: Option<String>,
        #[serde(default)]
        token_budget: Option<i64>,
    },
    PlanLifecycle {
        #[serde(default)]
        recorded_at: Option<i64>,
        #[serde(flatten)]
        lifecycle: PersistedPlanLifecycle,
    },
}

impl PersistedStructuredRolloutEvent {
    pub fn runtime_state_from_items(
        items: &[PersistedStructuredRolloutEvent],
        recorded_at: Option<i64>,
    ) -> Self {
        let mut explanation = None;
        let mut steps = Vec::new();
        let mut interactions = Vec::new();
        let mut plan_lifecycle = Vec::new();
        for item in items {
            match item {
                PersistedStructuredRolloutEvent::RuntimeState {
                    recorded_at: _,
                    explanation: item_explanation,
                    steps: item_steps,
                    interactions: item_interactions,
                    plan_lifecycle: item_plan_lifecycle,
                } => {
                    explanation = item_explanation.clone();
                    steps = item_steps.clone();
                    interactions = item_interactions.clone();
                    plan_lifecycle = item_plan_lifecycle.clone();
                }
                PersistedStructuredRolloutEvent::PlanState {
                    recorded_at: _,
                    explanation: item_explanation,
                    steps: item_steps,
                } => {
                    explanation = item_explanation.clone();
                    steps = item_steps.clone();
                }
                PersistedStructuredRolloutEvent::Interaction {
                    recorded_at: _,
                    interaction,
                } => {
                    interactions.push(interaction.clone());
                }
                PersistedStructuredRolloutEvent::PlanLifecycle {
                    recorded_at: _,
                    lifecycle,
                } => {
                    plan_lifecycle.push(lifecycle.clone());
                }
                PersistedStructuredRolloutEvent::Compaction { .. }
                | PersistedStructuredRolloutEvent::SpawnAgent { .. } => {}
            }
        }

        PersistedStructuredRolloutEvent::RuntimeState {
            recorded_at,
            explanation,
            steps,
            interactions,
            plan_lifecycle,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct PersistedLegacyRolloutMigration {
    pub structured_events: Vec<PersistedStructuredRolloutEvent>,
    pub runtime_rollout: Vec<PersistedRuntimeRolloutItem>,
    pub source: PersistedLegacyRolloutSource,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PersistedLegacyRolloutSource {
    StructuredLog,
    LegacyBackfilled,
    #[default]
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedThreadLineage {
    pub origin_kind: String,
    pub forked_from_thread_id: Option<String>,
}

impl Default for PersistedThreadLineage {
    fn default() -> Self {
        Self {
            origin_kind: "fresh".to_string(),
            forked_from_thread_id: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedThreadRecord {
    pub session_id: String,
    pub cwd: String,
    pub branch: String,
    pub provider: String,
    pub model: String,
    pub base_url: Option<String>,
    pub agent_mode: String,
    pub bash_approval: String,
    pub created_at: i64,
    pub lineage: PersistedThreadLineage,
    pub plan_explanation: Option<String>,
    pub history_len: usize,
    pub transcript_len: usize,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct PersistedRecentThreadSummary {
    pub session_id: String,
    pub provider: String,
    pub model: String,
    pub branch: String,
    pub updated_at: i64,
    pub preview: String,
    pub compaction_count: usize,
    pub last_compaction_before_tokens: Option<usize>,
    pub last_compaction_after_tokens: Option<usize>,
    pub last_compaction_recent_file_count: Option<usize>,
    pub last_compaction_boundary_version: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct PersistedRecentThreadRecord {
    pub session_id: String,
    pub cwd: String,
    pub branch: String,
    pub provider: String,
    pub model: String,
    pub base_url: Option<String>,
    pub agent_mode: String,
    pub bash_approval: String,
    pub created_at: i64,
    pub history_len: usize,
    pub transcript_len: usize,
    pub updated_at: i64,
    pub lineage: PersistedThreadLineage,
    pub preview: String,
    pub compaction_count: usize,
    pub last_compaction_before_tokens: Option<usize>,
    pub last_compaction_after_tokens: Option<usize>,
    pub last_compaction_recent_file_count: Option<usize>,
    pub last_compaction_boundary_version: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedSpawnAgentEdge {
    pub parent_session_id: String,
    pub event_id: String,
    pub agent_id: String,
    pub name: Option<String>,
    pub child_session_id: String,
    pub status: String,
    pub summary: Option<String>,
    pub token_budget: Option<i64>,
    pub recorded_at: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct PersistedCompactState {
    pub compaction_count: usize,
    pub last_compaction_before_tokens: Option<usize>,
    pub last_compaction_after_tokens: Option<usize>,
    pub last_compaction_recent_file_count: Option<usize>,
    pub last_compaction_boundary_version: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistedPromptRuntimeState {
    pub append_system_prompt: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PersistedSessionRuntimeState {
    pub agent_mode: String,
    pub bash_approval: String,
    pub prompt_runtime: PersistedPromptRuntimeState,
}

pub fn turn_preview(entries: &[PersistedTurnEntry]) -> String {
    entries
        .iter()
        .find_map(|entry| {
            let first_line = entry.message.lines().next()?.trim();
            if first_line.is_empty() {
                None
            } else {
                Some(format!("{}: {}", entry.role, first_line))
            }
        })
        .unwrap_or_else(|| "empty turn".to_string())
}
