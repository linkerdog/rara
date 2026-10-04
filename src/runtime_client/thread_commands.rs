use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use rara_state::state_db::StateDb;
use tokio::sync::oneshot;

use super::RuntimeClient;
use crate::agent::{Agent, AgentExecutionMode};
use crate::runtime_goals::{GoalStore, PreparedGoalRestore};
use crate::thread_io::ThreadIo;
use crate::thread_store::ThreadRecorder;
use crate::thread_store::ThreadRuntimeState;

pub(crate) struct ThreadCommandResult {
    pub source_session_id: String,
    pub outcome: ThreadCommandOutcome,
}

pub(crate) enum ThreadCommandOutcome {
    Exported {
        path: std::path::PathBuf,
    },
    Renamed {
        title: String,
    },
    Created {
        session_id: String,
        goal: PreparedGoalRestore,
    },
}

impl RuntimeClient {
    pub(crate) fn export_thread(
        &self,
        storage: &ThreadIo,
        db: Arc<StateDb>,
        path: Option<String>,
    ) -> Result<oneshot::Receiver<Result<ThreadCommandResult>>> {
        let agent = self.agent().context("runtime agent is not ready")?;
        let source_session_id = agent.session_id.clone();
        let legacy_root = agent.session_manager.legacy_storage_dir.clone();
        storage.execute(move || {
            let store = crate::thread_store::ThreadStore::new_for_roots(
                db.rollout_root(),
                legacy_root,
                &db,
            );
            let path = store.export_thread_file(&source_session_id, path.as_deref())?;
            Ok(ThreadCommandResult {
                source_session_id,
                outcome: ThreadCommandOutcome::Exported { path },
            })
        })
    }
    pub(crate) fn rename_thread(
        &self,
        storage: &ThreadIo,
        db: Arc<StateDb>,
        title: String,
    ) -> Result<oneshot::Receiver<Result<ThreadCommandResult>>> {
        let source_session_id = self
            .agent()
            .context("runtime agent is not ready")?
            .session_id
            .clone();
        storage.execute(move || {
            ThreadRecorder::new(&db).rename_thread(&source_session_id, &title)?;
            Ok(ThreadCommandResult {
                source_session_id,
                outcome: ThreadCommandOutcome::Renamed {
                    title: title.trim().to_owned(),
                },
            })
        })
    }

    pub(crate) fn create_thread(
        &self,
        storage: &ThreadIo,
        db: Arc<StateDb>,
    ) -> Result<oneshot::Receiver<Result<ThreadCommandResult>>> {
        ensure!(
            self.agent_activity_snapshots()?
                .iter()
                .all(|item| item.status != "running"),
            "wait for child agents to finish before starting a new thread"
        );
        let agent = self.agent().context("runtime agent is not ready")?;
        let source_session_id = agent.session_id.clone();
        let mode = match agent.execution_mode {
            AgentExecutionMode::Execute => "execute",
            AgentExecutionMode::Plan => "plan",
            AgentExecutionMode::Review => "review",
        };
        let prompt_runtime = rara_persistence::thread_data::PersistedPromptRuntimeState {
            append_system_prompt: agent.prompt_config().append_system_prompt.clone(),
            warnings: agent.prompt_config().warnings.clone(),
        };
        storage.execute(move || {
            let prior = rara_persistence::thread_metadata::load_thread_record(
                &db.rollout_root(),
                &source_session_id,
            )?
            .context("current thread checkpoint is missing")?;
            let session_id = uuid::Uuid::new_v4().to_string();
            let goal = GoalStore::prepare_restore(&session_id, db.clone())?;
            ThreadRecorder::new(&db).persist_runtime_state(&ThreadRuntimeState {
                session_id: &session_id,
                cwd: &prior.cwd,
                branch: &prior.branch,
                provider: &prior.provider,
                model: &prior.model,
                base_url: prior.base_url.as_deref(),
                agent_mode: mode,
                bash_approval: &prior.bash_approval,
                plan_explanation: None,
                prompt_runtime,
                history_len: 0,
                transcript_len: 0,
                compact_state: Default::default(),
            })?;
            Ok(ThreadCommandResult {
                source_session_id,
                outcome: ThreadCommandOutcome::Created { session_id, goal },
            })
        })
    }

    pub(crate) fn apply_new_thread(
        agent: &mut Agent,
        goal_handle: &crate::runtime_goals::GoalHandle,
        session_id: String,
        goal: PreparedGoalRestore,
    ) {
        agent.reset_for_new_thread(session_id);
        goal_handle.apply_prepared_restore(goal);
    }
}
