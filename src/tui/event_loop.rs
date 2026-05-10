use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crossterm::{event::EventStream, terminal::enable_raw_mode, terminal::size as terminal_size};
use futures::StreamExt;
use rara_state::state_db::StateDb;
use tokio::time::{Duration, MissedTickBehavior, interval};

use super::event_dispatch::dispatch_event;
use super::event_stream::{UiEvent, translate_event};
use super::maintainer::TuiMaintainer;
use super::render::{desired_viewport_height, render};
use super::session_restore::{restore_latest_thread, restore_thread_by_id};
use super::state::GoalHandle;
use super::state::ListPickerKind;
use super::state::Overlay;
use super::state::TuiApp;
use super::submit::clamp_command_palette_selection;
use super::terminal_ui::{
    build_terminal, handle_paste, teardown_terminal, update_terminal_viewport,
};
use crate::agent::Agent;
use crate::mcp_tool_cache::McpToolCache;
use crate::oauth::OAuthManager;
use crate::runtime_event_bus::RuntimeEventBus;

#[derive(Debug, Clone)]
pub enum StartupResumeTarget {
    Fresh,
    Latest,
    ThreadId(String),
    Picker,
}

use crate::mcp_connection_manager::McpConnectionManager;
use crate::protocol_sources::{PromptSourceRegistry, SkillSourceRegistry};

pub async fn run_tui(
    agent: Agent,
    goal_handle: GoalHandle,
    mcp_tool_cache: McpToolCache,
    oauth_manager: OAuthManager,
    startup_resume: StartupResumeTarget,
    sandbox_network_access: Arc<AtomicBool>,
    event_bus: Arc<RuntimeEventBus>,
    mcp_manager: Arc<McpConnectionManager>,
    prompt_source_registry: Arc<PromptSourceRegistry>,
    skill_source_registry: Arc<SkillSourceRegistry>,
) -> anyhow::Result<Option<String>> {
    enable_raw_mode()?;
    let initial_size = terminal_size()?;
    let mut app = TuiApp::new(crate::config::ConfigManager::new()?)?;
    app.goal_handle = goal_handle;
    app.goal = app.goal_handle.read().unwrap().clone();
    app.mcp_tool_cache = Some(mcp_tool_cache);
    app.sandbox_network_access = sandbox_network_access;
    app.event_bus = Some(event_bus.clone());
    app.mcp_manager = Some(mcp_manager);
    app.prompt_source_registry = Some(prompt_source_registry);
    app.skill_source_registry = Some(skill_source_registry);
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::new(event_bus.clone()),
    ));
    app.sandbox_network_access
        .store(false, std::sync::atomic::Ordering::Relaxed);
    app.terminal_width = initial_size.0;
    let viewport_height = desired_viewport_height(&app, initial_size.0, initial_size.1);
    let mut terminal = build_terminal(viewport_height)?;
    let mut maintainer = TuiMaintainer::new(app, Some(agent));
    match StateDb::new() {
        Ok(state_db) => {
            let state_db = Arc::new(state_db);
            let (app, agent_slot) = maintainer.split_mut();
            app.attach_state_db(state_db);
            match &startup_resume {
                StartupResumeTarget::Fresh => {
                    let _ = agent_slot;
                }
                StartupResumeTarget::Latest => {
                    if let Some(state_db) = app.state_db.as_ref().cloned() {
                        restore_latest_thread(&state_db, app, agent_slot)?;
                    }
                }
                StartupResumeTarget::ThreadId(thread_id) => {
                    restore_thread_by_id(thread_id.as_str(), app, agent_slot)?;
                }
                StartupResumeTarget::Picker => {
                    app.open_overlay(Overlay::ListPicker(ListPickerKind::Resume));
                }
            }
        }
        Err(err) => maintainer.app_mut().set_state_db_error(err.to_string()),
    }
    let oauth_manager = Arc::new(oauth_manager);
    maintainer.app_mut().codex_auth_mode = oauth_manager.saved_auth_mode().ok().flatten();
    let mut events = EventStream::new();
    let mut tick = interval(Duration::from_millis(166));
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);

    maintainer.sync_snapshot();
    maintainer.start_repo_context_detection();

    let result: anyhow::Result<()> = loop {
        maintainer.poll_repo_context().await;
        maintainer.poll_agent_task().await?;

        let needs_redraw = maintainer.needs_redraw;
        let (app, agent_slot) = maintainer.split_mut();
        let mut needs_redraw = needs_redraw;
        clamp_command_palette_selection(app);
        let size = terminal_size()?;
        app.terminal_width = size.0;
        let desired_height = desired_viewport_height(app, size.0, size.1);
        match update_terminal_viewport(&mut terminal, desired_height, app) {
            Ok(()) => {}
            Err(err) => app.push_notice(format!("Skipped viewport update: {err}")),
        }

        if needs_redraw {
            terminal.draw(|f| render(f, app))?;
            needs_redraw = false;
        }

        tokio::select! {
            _ = tick.tick() => {
                needs_redraw = true;
            }
            maybe_event = events.next() => {
                match maybe_event {
                    Some(Ok(event)) => match translate_event(event, app) {
                        Some(UiEvent::App(event)) => {
                            let _commands = super::app_command::commands_for_event(&event);
                            if dispatch_event(event, app, agent_slot, &oauth_manager).await? {
                                if let Some(task) = app.bottom_pane.running_task.take() {
                                    task.handle.abort();
                                }
                                break Ok(());
                            }
                            needs_redraw = true;
                        }
                        Some(UiEvent::Draw) => {
                            let size = terminal_size()?;
                            let desired_height = desired_viewport_height(app, size.0, size.1);
                            match update_terminal_viewport(&mut terminal, desired_height, app) {
                                Ok(()) => {}
                                Err(err) => app.push_notice(format!(
                                    "Skipped viewport redraw update: {err}"
                                )),
                            }
                            app.terminal_width = size.0;
                            let _ = terminal.clear_visible_screen();
                            needs_redraw = true;
                        }
                        Some(UiEvent::Paste(text)) => {
                            handle_paste(text, app);
                            needs_redraw = true;
                        }
                        Some(UiEvent::FocusChanged(_focused)) => {
                            if let Some(agent_ref) = agent_slot.as_ref() {
                                app.sync_snapshot(agent_ref);
                            }
                            needs_redraw = true;
                        }
                        None => {}
                    },
                    Some(Err(err)) => {
                        app.push_notice(format!("Terminal event error: {err}"));
                    }
                    None => break Ok(()),
                }
            }
        }
        let _ = agent_slot;
        let _ = app;
        maintainer.needs_redraw = needs_redraw;
    };
    if let Some(handle) = maintainer.app_mut().repo_context_task.take() {
        handle.abort();
    }
    teardown_terminal(terminal)?;
    result?;
    let session_id = maintainer
        .agent()
        .map(|a| a.session_id.clone())
        .filter(|id| !id.is_empty())
        .or_else(|| {
            (!maintainer.app().snapshot.session_id.is_empty())
                .then(|| maintainer.app().snapshot.session_id.clone())
        });
    Ok(session_id)
}
