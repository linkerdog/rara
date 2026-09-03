use std::path::PathBuf;
use std::sync::Arc;

use crossterm::{event::EventStream, terminal::enable_raw_mode, terminal::size as terminal_size};
use futures::StreamExt;
use rara_state::state_db::StateDb;
use tokio::time::{Duration, MissedTickBehavior, interval};

use super::controller::{RuntimeActivity, TuiController};
use super::event_stream::{UiEvent, translate_event};
use super::render::{desired_viewport_height, render};
use super::runtime::RuntimeCommandProcessor;
use super::runtime_port::{
    InProcessRuntimeClientPort, RuntimeClientPort, RuntimeCommand, RuntimeMaintenanceCommand,
};
use super::session_restore::{restore_latest_thread, restore_thread_by_id};
use super::state::ListPickerKind;
use super::state::Overlay;
use super::state::TuiApp;
use super::submit::clamp_command_palette_selection;
use super::terminal_ui::{
    build_terminal, handle_paste, teardown_terminal, update_terminal_viewport,
};
use crate::oauth::OAuthManager;
use crate::runtime_client::RuntimeClient;

#[derive(Debug, Clone)]
pub enum StartupResumeTarget {
    Fresh,
    Latest,
    ThreadId(String),
    Picker,
}

pub async fn run_tui(
    runtime: RuntimeClient,
    oauth_manager: OAuthManager,
    startup_resume: StartupResumeTarget,
) -> anyhow::Result<Option<String>> {
    enable_raw_mode()?;
    let initial_size = terminal_size()?;
    let mut app = TuiApp::new(crate::config::ConfigManager::new()?)?;
    app.goal_handle = runtime.goal_handle.clone();
    app.goal = runtime.goal_handle.read().unwrap().clone();
    app.mcp_tool_cache = Some(runtime.mcp_tool_cache.clone());
    app.sandbox_network_access = runtime.sandbox_network_access.clone();
    app.event_bus = Some(runtime.event_bus.clone());
    app.mcp_manager = Some(runtime.mcp_manager.clone());
    app.hook_runtime = Some(runtime.hook_runtime.clone());
    app.explicit_plugin_dirs = runtime.explicit_plugin_dirs.clone();
    app.lsp_manager = Some(runtime.lsp_manager.clone());
    app.memory_handler = Some(Arc::new(
        crate::protocol_sources::MemoryControlHandler::with_store(
            runtime.event_bus.clone(),
            runtime.agent().expect("runtime agent").memory_store.clone(),
        ),
    ));
    app.sandbox_network_access
        .store(false, std::sync::atomic::Ordering::Relaxed);
    app.terminal_width = initial_size.0;
    let viewport_height = desired_viewport_height(&app, initial_size.0, initial_size.1);
    let mut terminal = build_terminal(viewport_height)?;
    let mut processor = RuntimeCommandProcessor::new(runtime);
    let (runtime_port, runtime_commands) = InProcessRuntimeClientPort::new(
        processor.event_bus(),
        Arc::new(std::sync::RwLock::new(app.snapshot.clone())),
    );
    let runtime_port: Arc<dyn RuntimeClientPort> = Arc::new(runtime_port);
    let mut maintainer = TuiController::new(app, runtime_port, runtime_commands);
    match StateDb::new() {
        Ok(state_db) => {
            let state_db = Arc::new(state_db);
            let app = maintainer.app_mut();
            let agent_slot = processor.agent_mut();
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

    maintainer.sync_snapshot(&mut processor).await?;
    maintainer.start_repo_context_detection();
    if should_start_initial_rebuild(&maintainer.app().explicit_plugin_dirs) {
        maintainer
            .app_mut()
            .push_entry("Runtime", "Loading explicit plugin directories.");
        maintainer
            .send_runtime_command(RuntimeCommand::Maintenance(
                RuntimeMaintenanceCommand::Rebuild,
            ))
            .await?;
    }

    let result: anyhow::Result<()> = loop {
        let mut needs_redraw = maintainer.needs_redraw;
        if maintainer.poll_repo_context().await {
            needs_redraw = true;
        }
        {
            let app = maintainer.app_mut();
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
            if app.bottom_pane.check_paste_burst_flush() {
                needs_redraw = true;
            }
        }

        tokio::select! {
            _ = tick.tick() => {
                let mut changed = false;
                let app = maintainer.app_mut();
                if let Some(delta) = app.transcript_selection.autoscroll_delta() {
                    app.scroll_transcript(delta);
                    changed = true;
                }
                changed |= app.poll_shared_task_files();
                changed |= processor.sync_agent_activity(app);
                changed |= super::runtime::emit_query_heartbeat(app);
                needs_redraw |= changed;
            }
            runtime_activity = maintainer.wait_for_runtime_activity() => {
                match runtime_activity {
                    RuntimeActivity::Event(Some(event)) => {
                        needs_redraw |= maintainer.apply_runtime_event(event);
                        needs_redraw |= maintainer.complete_query_if_ready(&mut processor).await?;
                    }
                    RuntimeActivity::Event(None) => {}
                    RuntimeActivity::Completed(completion) => {
                        needs_redraw |= maintainer
                            .receive_runtime_task_completion(&mut processor, completion)
                            .await?;
                    }
                    RuntimeActivity::Command(Some(command)) => {
                        maintainer.apply_runtime_command(&mut processor, command).await?;
                        needs_redraw = true;
                    }
                    RuntimeActivity::Command(None) => {}
                }
            }
            maybe_event = events.next() => {
                match maybe_event {
                    Some(Ok(event)) => match translate_event(event, maintainer.app_mut()) {
                        Some(UiEvent::App(event)) => {
                            if maintainer
                                .dispatch_event(&mut processor, event, &oauth_manager)
                                .await?
                            {
                                if let Some(task) = maintainer.app_mut().bottom_pane.running_task.take() {
                                    task.handle.abort();
                                }
                                break Ok(());
                            }
                            needs_redraw = true;
                        }
                        Some(UiEvent::Draw) => {
                            let app = maintainer.app_mut();
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
                            let app = maintainer.app_mut();
                            handle_paste(text, app);
                            needs_redraw = true;
                        }
                        Some(UiEvent::FocusChanged(_focused)) => {
                            maintainer.sync_snapshot(&mut processor).await?;
                            maintainer.publish_snapshot_projection();
                            needs_redraw = true;
                        }
                        None => {}
                    },
                    Some(Err(err)) => {
                        maintainer
                            .app_mut()
                            .push_notice(format!("Terminal event error: {err}"));
                        needs_redraw = true;
                    }
                    None => break Ok(()),
                }
            }
        }
        maintainer.needs_redraw = needs_redraw;
    };
    if let Some(handle) = maintainer.app_mut().repo_context_task.take() {
        handle.abort();
    }
    teardown_terminal(terminal)?;
    if result.is_ok() {
        processor.drain_memory().await;
    }
    result?;
    let session_id = processor.session_id().or_else(|| {
        (!maintainer.app().snapshot.session_id.is_empty())
            .then(|| maintainer.app().snapshot.session_id.clone())
    });
    Ok(session_id)
}

fn should_start_initial_rebuild(explicit_plugin_dirs: &[PathBuf]) -> bool {
    !explicit_plugin_dirs.is_empty()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::should_start_initial_rebuild;

    #[test]
    fn initial_rebuild_starts_for_explicit_plugin_dirs() {
        assert!(should_start_initial_rebuild(&[PathBuf::from("/plugins")]));
        assert!(!should_start_initial_rebuild(&[]));
    }
}
