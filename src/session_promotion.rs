use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionShardPromotionTrigger {
    Periodic,
    Shutdown,
    RuntimeControl,
}

/// Reserved policy gate for periodic session shard promotion. Will be activated
/// by the scheduler tracked in docs/features/memory-records.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub struct SessionShardPromotionPolicy {
    pub enabled: bool,
    pub min_checkpoints: usize,
    pub max_checkpoints: usize,
}

impl Default for SessionShardPromotionPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            min_checkpoints: 2,
            max_checkpoints: 8,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionShardPromotionSkipReason {
    Disabled,
    Empty,
    BelowMinCheckpoints,
    MaxCheckpointsZero,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SessionShardPromotionDecision {
    Eligible,
    Skipped {
        reason: SessionShardPromotionSkipReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionShardPromotionPlan {
    pub session_id: String,
    pub trigger: SessionShardPromotionTrigger,
    pub checkpoint_count: usize,
    pub min_checkpoints: usize,
    pub max_checkpoints: usize,
    pub decision: SessionShardPromotionDecision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionShardPromotionOutcome {
    pub plan: SessionShardPromotionPlan,
    pub promoted_count: usize,
}

impl SessionShardPromotionPolicy {
    /// Reserved for scheduler-style promotion checks. Will be activated by the
    /// periodic promotion scheduler tracked in docs/features/memory-records.md.
    #[allow(dead_code)]
    pub fn evaluate(
        self,
        session_id: impl Into<String>,
        trigger: SessionShardPromotionTrigger,
        checkpoint_count: usize,
    ) -> SessionShardPromotionPlan {
        let decision = if !self.enabled {
            SessionShardPromotionDecision::Skipped {
                reason: SessionShardPromotionSkipReason::Disabled,
            }
        } else if self.max_checkpoints == 0 {
            SessionShardPromotionDecision::Skipped {
                reason: SessionShardPromotionSkipReason::MaxCheckpointsZero,
            }
        } else if checkpoint_count == 0 {
            SessionShardPromotionDecision::Skipped {
                reason: SessionShardPromotionSkipReason::Empty,
            }
        } else if checkpoint_count < self.min_checkpoints {
            SessionShardPromotionDecision::Skipped {
                reason: SessionShardPromotionSkipReason::BelowMinCheckpoints,
            }
        } else {
            SessionShardPromotionDecision::Eligible
        };

        SessionShardPromotionPlan {
            session_id: session_id.into(),
            trigger,
            checkpoint_count,
            min_checkpoints: self.min_checkpoints,
            max_checkpoints: self.max_checkpoints,
            decision,
        }
    }
}

impl SessionShardPromotionPlan {
    /// Reserved for scheduler-style promotion checks. Will be activated by the
    /// periodic promotion scheduler tracked in docs/features/memory-records.md.
    #[allow(dead_code)]
    pub fn is_eligible(&self) -> bool {
        matches!(self.decision, SessionShardPromotionDecision::Eligible)
    }
}
