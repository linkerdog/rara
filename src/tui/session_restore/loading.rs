use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use rara_state::state_db::{PersistedSessionRuntimeState, StateDb};
use tokio::sync::oneshot;

use super::{Agent, TranscriptEntry, TranscriptTurn, TuiApp, apply_prepared_restore};
use crate::runtime_goals::{GoalStore, PreparedGoalRestore};
use crate::session::SessionManager;
use crate::thread_store::{RolloutItem, ThreadSnapshot, ThreadStore};
use crate::todo::TodoState;
use crate::tui::event_loop::StartupResumeTarget;
use crate::tui::message_role::MessageRole;
use crate::tui::state::{ListPickerKind, NoticeLevel, Overlay, PermissionMode};

pub(crate) struct PendingRestore {
    source_session: String,
    receiver: oneshot::Receiver<Result<Option<PreparedRestore>>>,
}

pub(super) struct PreparedRestore {
    pub thread: ThreadSnapshot,
    pub todo_state: Option<TodoState>,
    pub runtime_state: Option<PersistedSessionRuntimeState>,
    pub turns: Vec<TranscriptTurn>,
    pub next_turn_ordinal: usize,
    pub live_entries: Vec<TranscriptEntry>,
    pub live_recovery_warning: Option<String>,
    pub latest_plan_lifecycle: Option<(String, Option<String>)>,
    pub goal: Result<PreparedGoalRestore, String>,
}

enum RestoreTarget {
    Latest,
    Thread(String),
}

pub(in crate::tui) fn apply_startup_resume(
    target: &StartupResumeTarget,
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
) {
    let target = match target {
        StartupResumeTarget::Fresh => return,
        StartupResumeTarget::Picker => {
            app.open_overlay(Overlay::ListPicker(ListPickerKind::Resume));
            return;
        }
        StartupResumeTarget::Latest => RestoreTarget::Latest,
        StartupResumeTarget::ThreadId(id) => RestoreTarget::Thread(id.clone()),
    };
    if let Err(error) = request_restore(target, app, agent_slot.as_ref()) {
        report_restore_error(app, error);
    }
}

pub(in crate::tui) fn request_restore_thread(
    thread_id: &str,
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
) -> Result<()> {
    request_restore(
        RestoreTarget::Thread(thread_id.into()),
        app,
        agent_slot.as_ref(),
    )
}

fn request_restore(target: RestoreTarget, app: &mut TuiApp, agent: Option<&Agent>) -> Result<()> {
    ensure!(
        !app.is_busy(),
        "wait for the active task before resuming a thread"
    );
    let agent = agent.context("runtime agent is not ready")?;
    let db = app
        .state_db
        .clone()
        .context("session storage is unavailable")?;
    let sessions = agent.session_manager.clone();
    let source_session = agent.session_id.clone();
    app.finalize_active_turn();
    app.persist_runtime_state();
    let storage = app
        .storage
        .as_ref()
        .context("session storage worker is unavailable")?;
    let receiver = storage.read(move || prepare_restore(target, &sessions, db))?;
    app.pending_restore = Some(PendingRestore {
        source_session,
        receiver,
    });
    app.push_notice(
        NoticeLevel::Info,
        "Loading saved thread... Press Esc to cancel.",
    );
    Ok(())
}

fn prepare_restore(
    target: RestoreTarget,
    sessions: &SessionManager,
    db: Arc<StateDb>,
) -> Result<Option<PreparedRestore>> {
    let store = ThreadStore::new(sessions, &db);
    let thread_id = match target {
        RestoreTarget::Thread(id) => id,
        RestoreTarget::Latest => {
            let Some(summary) = store.latest_thread_summary()? else {
                return Ok(None);
            };
            summary.metadata.session_id
        }
    };
    let mut thread = store.load_thread(&thread_id)?;
    let todo_state = sessions.load_todo_state(&thread_id)?;
    let runtime_state = db.load_session_runtime_state(&thread_id)?;
    let live_log = rara_persistence::thread_turn_log::load_live_entries_with_recovery(
        &db.rollout_root(),
        &thread_id,
    );
    let live_recovery_warning = live_log.warning();
    let live_entries = live_log
        .entries
        .into_iter()
        .map(|entry| TranscriptEntry::new(MessageRole::from_persisted(&entry.role), entry.message))
        .collect();
    let latest_plan_lifecycle = thread
        .rollout_items
        .iter()
        .rev()
        .find_map(|item| match item {
            RolloutItem::PlanLifecycle(lifecycle) => {
                Some((lifecycle.phase.clone(), lifecycle.tool_use_id.clone()))
            }
            _ => None,
        });
    let next_turn_ordinal = thread
        .rollout_items
        .iter()
        .filter_map(|item| match item {
            RolloutItem::Turn(turn) => Some(turn.summary.ordinal.saturating_add(1)),
            RolloutItem::Compaction(_)
            | RolloutItem::PlanState { .. }
            | RolloutItem::Interaction(_)
            | RolloutItem::PlanLifecycle(_)
            | RolloutItem::SpawnAgent { .. } => None,
        })
        .max()
        .unwrap_or(0);
    let turns = std::mem::take(&mut thread.rollout_items)
        .into_iter()
        .filter_map(|item| match item {
            RolloutItem::Turn(turn) if !turn.entries.is_empty() => Some(TranscriptTurn {
                thinking_duration: None,
                entries: turn
                    .entries
                    .into_iter()
                    .map(|entry| {
                        TranscriptEntry::new(
                            MessageRole::from_persisted(&entry.role),
                            entry.message,
                        )
                    })
                    .collect(),
            }),
            RolloutItem::Turn(_)
            | RolloutItem::Compaction(_)
            | RolloutItem::PlanState { .. }
            | RolloutItem::Interaction(_)
            | RolloutItem::PlanLifecycle(_)
            | RolloutItem::SpawnAgent { .. } => None,
        })
        .collect();
    let goal = GoalStore::prepare_restore(&thread_id, db).map_err(|error| format!("{error:#}"));
    Ok(Some(PreparedRestore {
        thread,
        todo_state,
        runtime_state,
        turns,
        next_turn_ordinal,
        live_entries,
        live_recovery_warning,
        latest_plan_lifecycle,
        goal,
    }))
}

pub(in crate::tui) fn poll_restore(app: &mut TuiApp, agent_slot: &mut Option<Agent>) -> bool {
    let Some(pending) = app.pending_restore.as_mut() else {
        return false;
    };
    let result = match pending.receiver.try_recv() {
        Ok(result) => result,
        Err(oneshot::error::TryRecvError::Empty) => return false,
        Err(oneshot::error::TryRecvError::Closed) => Err(anyhow::anyhow!(
            "storage worker stopped while loading the thread"
        )),
    };
    let source_session = pending.source_session.clone();
    app.pending_restore = None;
    if let Err(error) = finish_restore(result, &source_session, app, agent_slot) {
        report_restore_error(app, error);
    }
    true
}

fn finish_restore(
    result: Result<Option<PreparedRestore>>,
    source_session: &str,
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
) -> Result<()> {
    let pending_mode = app.pending_permission_mode.take();
    let result = (|| {
        let prepared = result?;
        let agent = agent_slot.as_mut().context("runtime agent is not ready")?;
        ensure!(
            agent.session_id == source_session,
            "discarded a restore result for a replaced session"
        );
        if let Some(prepared) = prepared {
            apply_prepared_restore(prepared, app, agent)?;
            if app.permission_mode == PermissionMode::FullAccess {
                crate::tui::runtime::request_permission_mode(
                    app,
                    agent_slot,
                    PermissionMode::FullAccess,
                );
            }
            if app.overlay == Some(Overlay::ListPicker(ListPickerKind::Resume)) {
                app.dismiss_overlay();
            }
        } else {
            app.push_notice(NoticeLevel::Info, "No saved thread found.");
        }
        Ok(())
    })();
    if let Some(mode) = pending_mode {
        crate::tui::runtime::request_permission_mode(app, agent_slot, mode);
    }
    result
}

pub(in crate::tui) fn cancel_restore(app: &mut TuiApp, slot: &mut Option<Agent>) -> bool {
    if app.pending_restore.take().is_none() {
        return false;
    }
    if let Some(mode) = app.pending_permission_mode.take() {
        crate::tui::runtime::request_permission_mode(app, slot, mode);
    }
    true
}

fn report_restore_error(app: &mut TuiApp, error: anyhow::Error) {
    log::warn!("Could not resume thread: {error:#}");
    app.push_notice(
        NoticeLevel::Error,
        format!("Could not resume thread; keeping the current session: {error:#}"),
    );
}

#[cfg(test)]
pub(in crate::tui) async fn finish_restore_for_test(
    app: &mut TuiApp,
    slot: &mut Option<Agent>,
) -> Result<()> {
    let pending = app.pending_restore.take().context("pending restore")?;
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), pending.receiver)
        .await
        .context("restore timeout")?
        .context("restore worker")?;
    let result = finish_restore(result, &pending.source_session, app, slot);
    if let Err(error) = &result {
        report_restore_error(app, anyhow::anyhow!("{error:#}"));
    }
    result
}

#[cfg(test)]
pub(in crate::tui) async fn restore_thread_by_id(
    id: &str,
    app: &mut TuiApp,
    slot: &mut Option<Agent>,
) -> Result<()> {
    request_restore_thread(id, app, slot)?;
    finish_restore_for_test(app, slot).await
}

#[cfg(test)]
pub(in crate::tui) async fn restore_latest_thread(
    _db: &Arc<StateDb>,
    app: &mut TuiApp,
    slot: &mut Option<Agent>,
) -> Result<()> {
    request_restore(RestoreTarget::Latest, app, slot.as_ref())?;
    finish_restore_for_test(app, slot).await
}
