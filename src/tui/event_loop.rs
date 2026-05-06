use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crossterm::{event::EventStream, terminal::enable_raw_mode, terminal::size as terminal_size};
use futures::StreamExt;
use tokio::time::{Duration, MissedTickBehavior, interval};

use super::event_dispatch::dispatch_event;
use super::event_stream::{UiEvent, translate_event};
use super::render::{desired_viewport_height, render};
use super::runtime::finish_running_task_if_ready;
use super::session_restore::{restore_latest_thread, restore_thread_by_id};
use super::state::GoalHandle;
use super::state::ListPickerKind;
use super::state::Overlay;
use super::state::TuiApp;
use super::submit::clamp_command_palette_selection;
use super::terminal_ui::{
    build_terminal, flush_committed_history, handle_paste, teardown_terminal,
    update_terminal_viewport,
};
use crate::agent::Agent;
use crate::oauth::OAuthManager;
use crate::runtime_event_bus::RuntimeEventBus;
use crate::state_db::StateDb;

#[derive(Debug, Clone)]
pub enum StartupResumeTarget {
    Fresh,
    Latest,
    ThreadId(String),
    Picker,
}

pub async fn run_tui(
    agent: Agent,
    goal_handle: GoalHandle,
    oauth_manager: OAuthManager,
    startup_resume: StartupResumeTarget,
    sandbox_network_access: Arc<AtomicBool>,
    event_bus: Arc<RuntimeEventBus>,
) -> anyhow::Result<Option<String>> {
    enable_raw_mode()?;
    let initial_size = terminal_size()?;
    let mut app = TuiApp::new(crate::config::ConfigManager::new()?)?;
    app.goal_handle = goal_handle;
    app.goal = app.goal_handle.read().unwrap().clone();
    app.sandbox_network_access = sandbox_network_access;
    app.event_bus = Some(event_bus);
    // Sync the AtomicBool to match the default Auto preset (network off).
    app.sandbox_network_access
        .store(false, std::sync::atomic::Ordering::Relaxed);
    app.terminal_width = initial_size.0;
    let viewport_height = desired_viewport_height(&app, initial_size.0, initial_size.1);
    let mut terminal = build_terminal(viewport_height)?;
    let mut agent_slot = Some(agent);
    match StateDb::new() {
        Ok(state_db) => {
            let state_db = Arc::new(state_db);
            app.attach_state_db(state_db);
            match &startup_resume {
                StartupResumeTarget::Fresh => {}
                StartupResumeTarget::Latest => {
                    if let Some(state_db) = app.state_db.as_ref().cloned() {
                        restore_latest_thread(&state_db, &mut app, &mut agent_slot)?;
                    }
                }
                StartupResumeTarget::ThreadId(thread_id) => {
                    restore_thread_by_id(thread_id.as_str(), &mut app, &mut agent_slot)?;
                }
                StartupResumeTarget::Picker => {
                    app.open_overlay(Overlay::ListPicker(ListPickerKind::Resume));
                }
            }
        }
        Err(err) => app.set_state_db_error(err.to_string()),
    }
    let oauth_manager = Arc::new(oauth_manager);
    app.codex_auth_mode = oauth_manager.saved_auth_mode().ok().flatten();
    let mut events = EventStream::new();
    let mut tick = interval(Duration::from_millis(166));
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);

    if let Some(agent_ref) = agent_slot.as_ref() {
        app.sync_snapshot(agent_ref);
    }
    app.start_repo_context_detection();

    let result: anyhow::Result<()> = loop {
        app.finish_repo_context_task_if_ready().await;
        finish_running_task_if_ready(&mut app, &mut agent_slot).await?;
        clamp_command_palette_selection(&mut app);
        let size = terminal_size()?;
        app.terminal_width = size.0;
        let desired_height = desired_viewport_height(&app, size.0, size.1);
        match update_terminal_viewport(&mut terminal, desired_height, &mut app) {
            Ok(()) => {}
            Err(err) => app.push_notice(format!("Skipped viewport update: {err}")),
        }
        flush_committed_history(&mut terminal, &mut app)?;
        // flush_committed_history writes lines above the viewport via crossterm
        // (DECSTBM scroll regions / raw Print), bypassing ratatui's double-buffer.
        // Clear the viewport area on the terminal and reset the diff buffer so
        // the next draw pass does a full repaint instead of diffing against a
        // stale buffer that doesn't match the real screen.
        terminal.clear_and_invalidate_viewport()?;
        terminal.draw(|f| render(f, &app))?;

        tokio::select! {
            _ = tick.tick() => {}
            maybe_event = events.next() => {
                match maybe_event {
                    Some(Ok(event)) => match translate_event(event, &app) {
                        Some(UiEvent::App(event)) => {
                            if dispatch_event(event, &mut app, &mut agent_slot, &oauth_manager).await? {
                                if let Some(task) = app.running_task.take() {
                                    task.handle.abort();
                                }
                                break Ok(());
                            }
                        }
                        Some(UiEvent::Draw) => {
                            let size = terminal_size()?;
                            let desired_height = desired_viewport_height(&app, size.0, size.1);
                            match update_terminal_viewport(&mut terminal, desired_height, &mut app) {
                                Ok(()) => {}
                                Err(err) => app.push_notice(format!("Skipped viewport redraw update: {err}")),
                            }
                        }
                        Some(UiEvent::Paste(text)) => {
                            handle_paste(text, &mut app);
                        }
                        Some(UiEvent::FocusChanged(focused)) => {
                            app.terminal_focused = focused;
                        }
                        None => {}
                    },
                    Some(Err(err)) => break Err(err.into()),
                    None => break Ok(()),
                }
            }
        }
    };

    if let Some(handle) = app.repo_context_task.take() {
        handle.abort();
    }
    teardown_terminal(terminal)?;
    result?;

    let session_id = agent_slot
        .as_ref()
        .map(|agent| agent.session_id.clone())
        .filter(|session_id| !session_id.is_empty())
        .or_else(|| (!app.snapshot.session_id.is_empty()).then(|| app.snapshot.session_id.clone()));
    Ok(session_id)
}
