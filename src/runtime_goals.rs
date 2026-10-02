use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{Context, Result};
use rara_state::state_db::StateDb;
use serde::{Deserialize, Serialize};

/// Represents the lifecycle state of a ralph loop goal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GoalStatus {
    /// Agent is actively working toward the goal across turns.
    Pursuing,
    /// User paused the goal; can be resumed.
    Paused,
    /// Agent reported a genuine blocker after repeated attempts to resolve it.
    Blocked,
    /// Goal was completed successfully.
    Complete,
    /// Goal exceeded its configured token budget; soft-stop.
    BudgetLimited,
}

/// Tracks a long-running objective that the agent autonomously works toward.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RalphGoal {
    /// The objective text set by `/goal <objective>`.
    pub objective: String,
    /// Current lifecycle status.
    pub status: GoalStatus,
    /// Optional token budget (input tokens). None = unlimited.
    pub token_budget: Option<u32>,
    /// Total input tokens consumed by goal turns.
    pub tokens_used: u32,
    /// Number of autonomous turns completed toward this goal.
    pub turns_completed: u32,
    /// Unix timestamp in seconds when the goal was created.
    pub created_at_epoch_seconds: u64,
}

impl RalphGoal {
    pub fn new(objective: String, token_budget: Option<u32>) -> Self {
        Self {
            objective,
            status: GoalStatus::Pursuing,
            token_budget,
            tokens_used: 0,
            turns_completed: 0,
            created_at_epoch_seconds: current_unix_timestamp_secs(),
        }
    }

    pub fn time_used_seconds(&self) -> u64 {
        current_unix_timestamp_secs().saturating_sub(self.created_at_epoch_seconds)
    }

    pub fn remaining_tokens(&self) -> Option<u32> {
        self.token_budget
            .map(|budget| budget.saturating_sub(self.tokens_used))
    }
}

pub fn current_unix_timestamp_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

pub type GoalHandle = Arc<GoalStore>;

/// Serializes goal mutations and publishes state only after its durable write succeeds.
#[derive(Default)]
pub struct GoalStore {
    state: Mutex<GoalState>,
}

#[derive(Clone, Default)]
struct GoalState {
    goal: Option<RalphGoal>,
    persistence: GoalPersistence,
}

#[derive(Clone, Default)]
enum GoalPersistence {
    #[default]
    InMemory,
    Durable {
        db: Arc<StateDb>,
        thread_id: String,
    },
    Unavailable {
        reason: String,
    },
}

impl GoalStore {
    fn locked(&self) -> MutexGuard<'_, GoalState> {
        self.state.lock().unwrap_or_else(|poisoned| {
            log::warn!("Goal state mutex was poisoned; recovering the last committed snapshot");
            poisoned.into_inner()
        })
    }

    pub fn snapshot(&self) -> Option<RalphGoal> {
        self.locked().goal.clone()
    }

    pub fn restore_for_thread(
        &self,
        thread_id: &str,
        db: Arc<StateDb>,
    ) -> Result<Option<RalphGoal>> {
        let mut state = self.locked();
        let goal = db
            .try_load_goal(thread_id)?
            .map(serde_json::from_value::<RalphGoal>)
            .transpose()
            .with_context(|| format!("invalid persisted goal for thread {thread_id}"))?;
        state.goal = goal.clone();
        state.persistence = GoalPersistence::Durable {
            db,
            thread_id: thread_id.into(),
        };
        Ok(goal)
    }

    pub(crate) fn disable_after_persistence_failure(&self, reason: String) {
        let mut state = self.locked();
        state.goal = None;
        state.persistence = GoalPersistence::Unavailable { reason };
    }

    pub fn mutate<R>(&self, change: impl FnOnce(&mut Option<RalphGoal>) -> Result<R>) -> Result<R> {
        let mut state = self.locked();
        let mut next = state.goal.clone();
        let result = change(&mut next)?;
        if next == state.goal {
            return Ok(result);
        }
        match &state.persistence {
            GoalPersistence::InMemory => {}
            GoalPersistence::Durable { db, thread_id } => match &next {
                Some(goal) => db.save_goal(thread_id, &serde_json::to_value(goal)?)?,
                None => db.delete_goal(thread_id)?,
            },
            GoalPersistence::Unavailable { reason } => {
                anyhow::bail!("goal persistence unavailable: {reason}");
            }
        }
        state.goal = next;
        Ok(result)
    }

    pub fn replace(&self, goal: Option<RalphGoal>) -> Result<()> {
        self.mutate(|stored| {
            *stored = goal;
            Ok(())
        })
    }

    pub fn inherit_from(&self, previous: &Self) {
        let previous = previous.locked().clone();
        *self.locked() = previous;
    }
}

#[cfg(test)]
#[path = "runtime_goals_tests.rs"]
mod tests;
