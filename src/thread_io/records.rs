use anyhow::{Context, Result};
use rara_persistence::thread_data::{
    PersistedCompactState, PersistedInteraction, PersistedPlanStep, PersistedPromptRuntimeState,
    PersistedStructuredRolloutEvent, PersistedTurnEntry,
};
use rara_persistence::thread_turn_log;
use rara_state::state_db::StateDb;

use crate::thread_store::{ThreadRecorder, ThreadRuntimeState};

pub(crate) struct RuntimeCheckpoint {
    pub session_id: String,
    pub cwd: String,
    pub branch: String,
    pub provider: String,
    pub model: String,
    pub base_url: Option<String>,
    pub agent_mode: String,
    pub bash_approval: String,
    pub plan_explanation: Option<String>,
    pub prompt_runtime: PersistedPromptRuntimeState,
    pub history_len: usize,
    pub transcript_len: usize,
    pub compact_state: PersistedCompactState,
    pub plan_steps: Vec<PersistedPlanStep>,
    pub interactions: Vec<PersistedInteraction>,
    pub rollout: Vec<PersistedStructuredRolloutEvent>,
}

pub(crate) enum WriteOperation {
    Runtime(Box<RuntimeCheckpoint>),
    AppendLive {
        session_id: String,
        entries: Vec<PersistedTurnEntry>,
    },
    ReplaceLive {
        session_id: String,
        entries: Vec<PersistedTurnEntry>,
    },
    CommitTurn {
        session_id: String,
        ordinal: usize,
        entries: Vec<PersistedTurnEntry>,
    },
    ClearLive {
        session_id: String,
    },
}

impl WriteOperation {
    pub(super) fn merge(&mut self, next: Self) -> Result<(), Self> {
        match (self, next) {
            (Self::Runtime(current), Self::Runtime(next))
                if current.session_id == next.session_id =>
            {
                *current = next;
                Ok(())
            }
            (
                Self::AppendLive {
                    session_id,
                    entries,
                },
                Self::AppendLive {
                    session_id: next_session,
                    entries: next_entries,
                },
            ) if *session_id == next_session => {
                entries.extend(next_entries);
                Ok(())
            }
            (_, next) => Err(next),
        }
    }

    pub(super) fn execute(&self, db: &StateDb) -> Result<()> {
        let recorder = ThreadRecorder::new(db);
        let root = db.rollout_root();
        match self {
            Self::Runtime(state) => {
                recorder
                    .persist_runtime_state(&ThreadRuntimeState {
                        session_id: &state.session_id,
                        cwd: &state.cwd,
                        branch: &state.branch,
                        provider: &state.provider,
                        model: &state.model,
                        base_url: state.base_url.as_deref(),
                        agent_mode: &state.agent_mode,
                        bash_approval: &state.bash_approval,
                        plan_explanation: state.plan_explanation.as_deref(),
                        prompt_runtime: state.prompt_runtime.clone(),
                        history_len: state.history_len,
                        transcript_len: state.transcript_len,
                        compact_state: state.compact_state.clone(),
                    })
                    .context("runtime checkpoint write failed")?;
                recorder
                    .replace_plan_steps(&state.session_id, &state.plan_steps)
                    .context("plan write failed")?;
                recorder
                    .replace_interactions(&state.session_id, &state.interactions)
                    .context("interaction write failed")?;
                recorder
                    .replace_runtime_rollout_events(&state.session_id, &state.rollout)
                    .context("structured rollout write failed")
            }
            Self::AppendLive {
                session_id,
                entries,
            } => thread_turn_log::append_rollout_fragments(&root, session_id, entries)
                .context("live transcript write failed"),
            Self::ReplaceLive {
                session_id,
                entries,
            } => thread_turn_log::replace_live_entries(&root, session_id, entries)
                .context("live transcript rewrite failed"),
            Self::CommitTurn {
                session_id,
                ordinal,
                entries,
            } => {
                recorder
                    .persist_turn(session_id, *ordinal, entries)
                    .context("turn write failed")?;
                // Never remove the recovery copy until the canonical turn and
                // its index are durable. Retrying the ordinal is idempotent.
                thread_turn_log::clear_live_log(&root, session_id).context("live log clear failed")
            }
            Self::ClearLive { session_id } => {
                thread_turn_log::clear_live_log(&root, session_id).context("live log clear failed")
            }
        }
    }
}
