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

/// Captures goal membership and input-token usage before a query begins.
pub struct GoalTurn {
    membership: Arc<()>,
    prior_input_tokens: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GoalContinuationMode {
    Automatic,
    Requested,
}

/// Identifies one restored goal before another turn or lifecycle change.
#[derive(Clone, Debug)]
pub(crate) struct GoalResumeTicket(Arc<()>);

impl PartialEq for GoalResumeTicket {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Deserialize)]
struct StoredGoal {
    #[serde(flatten)]
    goal: RalphGoal,
    #[serde(default)]
    continuation_deferred: bool,
}

/// Serializes goal mutations and publishes state only after its durable write succeeds.
#[derive(Default)]
pub struct GoalStore {
    state: Mutex<GoalState>,
}

#[derive(Clone, Default)]
struct GoalState {
    goal: Option<RalphGoal>,
    persistence: GoalPersistence,
    continuation_deferred: bool,
    admission: Arc<()>,
    membership: Arc<()>,
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

pub(crate) struct PreparedGoalRestore(GoalState);

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

    pub fn begin_turn(&self, prior_input_tokens: u32) -> Option<GoalTurn> {
        let state = self.locked();
        state
            .goal
            .as_ref()
            .filter(|goal| goal.status == GoalStatus::Pursuing)
            .map(|_| GoalTurn {
                membership: state.membership.clone(),
                prior_input_tokens,
            })
    }

    pub fn restore_for_thread(
        &self,
        thread_id: &str,
        db: Arc<StateDb>,
    ) -> Result<Option<RalphGoal>> {
        let mut state = self.locked();
        let prepared = Self::prepare_restore(thread_id, db)?;
        *state = prepared.0;
        Ok(state.goal.clone())
    }

    pub(crate) fn prepare_restore(
        thread_id: &str,
        db: Arc<StateDb>,
    ) -> Result<PreparedGoalRestore> {
        let stored = db
            .try_load_goal(thread_id)?
            .map(serde_json::from_value::<StoredGoal>)
            .transpose()
            .with_context(|| format!("invalid persisted goal for thread {thread_id}"))?;
        let deferred = stored
            .as_ref()
            .is_some_and(|stored| stored.continuation_deferred);
        let goal = stored.map(|stored| stored.goal);
        if let Some(goal) = goal.as_ref() {
            anyhow::ensure!(
                !goal.objective.trim().is_empty(),
                "invalid persisted goal for thread {thread_id}: objective must not be empty"
            );
            anyhow::ensure!(
                goal.token_budget != Some(0),
                "invalid persisted goal for thread {thread_id}: token budget must be positive"
            );
        }
        Ok(PreparedGoalRestore(GoalState {
            goal,
            continuation_deferred: deferred,
            persistence: GoalPersistence::Durable {
                db,
                thread_id: thread_id.into(),
            },
            ..GoalState::default()
        }))
    }

    pub(crate) fn apply_prepared_restore(
        &self,
        prepared: PreparedGoalRestore,
    ) -> Option<RalphGoal> {
        let mut state = self.locked();
        *state = prepared.0;
        state.goal.clone()
    }

    pub(crate) fn disable_after_persistence_failure(&self, reason: String) {
        let mut state = self.locked();
        state.goal = None;
        state.admission = Arc::new(());
        state.membership = Arc::new(());
        state.persistence = GoalPersistence::Unavailable { reason };
    }

    pub fn mutate<R>(&self, change: impl FnOnce(&mut Option<RalphGoal>) -> Result<R>) -> Result<R> {
        self.mutate_for_turn(None, |stored, _| change(stored))
    }

    pub(crate) fn mutate_for_turn<R>(
        &self,
        turn: Option<&GoalTurn>,
        change: impl FnOnce(&mut Option<RalphGoal>, Option<u32>) -> Result<R>,
    ) -> Result<R> {
        let mut state = self.locked();
        let prior_input_tokens = turn
            .filter(|turn| Arc::ptr_eq(&turn.membership, &state.membership))
            .map(|turn| turn.prior_input_tokens);
        let mut next = state.goal.clone();
        let result = change(&mut next, prior_input_tokens)?;
        if next == state.goal {
            return Ok(result);
        }
        let same_goal = match (&state.goal, &next) {
            (Some(previous), Some(next)) => {
                previous.objective == next.objective
                    && previous.created_at_epoch_seconds == next.created_at_epoch_seconds
                    && !(previous.status == GoalStatus::Complete
                        && next.status == GoalStatus::Pursuing)
            }
            (None, None) => true,
            (None, Some(_)) | (Some(_), None) => false,
        };
        let deferred = same_goal && state.continuation_deferred;
        Self::persist(&state, &next, deferred)?;
        state.continuation_deferred = deferred;
        state.admission = Arc::new(());
        if !same_goal {
            state.membership = Arc::new(());
        }
        state.goal = next;
        Ok(result)
    }

    fn persist(state: &GoalState, goal: &Option<RalphGoal>, deferred: bool) -> Result<()> {
        match &state.persistence {
            GoalPersistence::InMemory => Ok(()),
            GoalPersistence::Durable { db, thread_id } => match goal {
                Some(goal) => {
                    let mut row = serde_json::to_value(goal)?;
                    row["continuation_deferred"] = deferred.into();
                    db.save_goal(thread_id, &row)
                }
                None => db.delete_goal(thread_id),
            },
            GoalPersistence::Unavailable { reason } => {
                anyhow::bail!("goal persistence unavailable: {reason}")
            }
        }
    }

    pub(crate) fn continuation_deferred(&self) -> bool {
        self.locked().continuation_deferred
    }

    pub(crate) fn defer_continuation(&self) -> Result<()> {
        let mut state = self.locked();
        // Invalidate queued admission even when the durable stop cannot be saved.
        state.admission = Arc::new(());
        if state.goal.is_none() || state.continuation_deferred {
            return Ok(());
        }
        Self::persist(&state, &state.goal, true)?;
        state.continuation_deferred = true;
        Ok(())
    }

    pub(crate) fn record_turn_started(&self) -> Result<()> {
        let mut state = self.locked();
        state.admission = Arc::new(());
        if state.continuation_deferred {
            Self::persist(&state, &state.goal, false)?;
            state.continuation_deferred = false;
        }
        Ok(())
    }

    pub(crate) fn resume_ticket(&self) -> Option<GoalResumeTicket> {
        let state = self.locked();
        state
            .goal
            .as_ref()
            .map(|_| GoalResumeTicket(state.admission.clone()))
    }

    pub(crate) fn matches_resume_ticket(&self, ticket: &GoalResumeTicket) -> bool {
        Arc::ptr_eq(&self.locked().admission, &ticket.0)
    }

    pub(crate) fn edit_objective(
        &self,
        ticket: &GoalResumeTicket,
        objective: String,
    ) -> Result<()> {
        let mut state = self.locked();
        anyhow::ensure!(
            Arc::ptr_eq(&state.admission, &ticket.0),
            "goal changed; reopen the editor"
        );
        anyhow::ensure!(
            !objective.trim().is_empty(),
            "goal objective must not be empty"
        );
        let mut goal = state.goal.clone().context("no goal to edit")?;
        goal.objective = objective;
        Self::persist(&state, &Some(goal.clone()), state.continuation_deferred)?;
        state.goal = Some(goal);
        state.admission = Arc::new(());
        Ok(())
    }

    pub(crate) fn replace_confirmed(
        &self,
        ticket: &GoalResumeTicket,
        goal: RalphGoal,
    ) -> Result<()> {
        let mut state = self.locked();
        anyhow::ensure!(
            Arc::ptr_eq(&state.admission, &ticket.0),
            "goal changed; request replacement again"
        );
        Self::persist(&state, &Some(goal.clone()), false)?;
        state.goal = Some(goal);
        state.continuation_deferred = false;
        state.admission = Arc::new(());
        state.membership = Arc::new(());
        Ok(())
    }

    /// Claim once under the goal lock and enforce the budget before generating work.
    pub(crate) fn claim_continuation(
        &self,
        ticket: &GoalResumeTicket,
        mode: GoalContinuationMode,
    ) -> Result<Option<RalphGoal>> {
        let mut state = self.locked();
        if !Arc::ptr_eq(&state.admission, &ticket.0)
            || (mode == GoalContinuationMode::Automatic && state.continuation_deferred)
        {
            return Ok(None);
        }
        let Some(mut goal) = state.goal.clone() else {
            return Ok(None);
        };
        match goal.status {
            GoalStatus::Pursuing => {}
            GoalStatus::BudgetLimited if mode == GoalContinuationMode::Requested => {}
            GoalStatus::Paused
            | GoalStatus::Blocked
            | GoalStatus::Complete
            | GoalStatus::BudgetLimited => return Ok(None),
        }
        if goal
            .token_budget
            .is_some_and(|budget| goal.tokens_used >= budget)
        {
            goal.status = GoalStatus::BudgetLimited;
        }
        if state.goal.as_ref() != Some(&goal) || state.continuation_deferred {
            Self::persist(&state, &Some(goal.clone()), false)?;
            state.goal = Some(goal.clone());
            state.continuation_deferred = false;
        }
        state.admission = Arc::new(());
        Ok(Some(goal))
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
