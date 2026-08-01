use std::path::PathBuf;
use std::sync::Arc;

use crossterm::{event::EventStream, terminal::enable_raw_mode, terminal::size as terminal_size};
use futures::StreamExt;
use rara_state::state_db::StateDb;
use tokio::time::{Duration, MissedTickBehavior, interval};

use super::event_dispatch::dispatch_event;
use super::event_stream::{UiEvent, translate_event};
use super::maintainer::TuiMaintainer;
use super::render::{desired_viewport_height, render};
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
    initialize_local_embeddings: bool,
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
    app.prompt_source_registry = Some(runtime.prompt_source_registry.clone());
    app.skill_source_registry = Some(runtime.skill_source_registry.clone());
    app.hook_registry = Some(runtime.hook_registry.clone());
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
    let mut maintainer = TuiMaintainer::new(app, runtime);
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
    if should_start_initial_rebuild(
        initialize_local_embeddings,
        &maintainer.app().explicit_plugin_dirs,
    ) {
        let (app, _) = maintainer.split_mut();
        if initialize_local_embeddings {
            app.push_entry("Runtime", "Initializing local embedding model.");
        } else {
            app.push_entry("Runtime", "Loading explicit plugin directories.");
        }
        super::runtime::start_rebuild_task(app);
    }

    let result: anyhow::Result<()> = loop {
        maintainer.poll_repo_context().await;
        maintainer.poll_agent_task().await?;

        let mut needs_redraw = maintainer.needs_redraw;
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
                let app = maintainer.app_mut();
                if let Some(delta) = app.transcript_selection.autoscroll_delta() {
                    app.scroll_transcript(delta);
                }
                let _ = app.poll_shared_task_files();
                needs_redraw = true;
            }
            maybe_agent_event = maintainer.wait_for_agent_event() => {
                if let Some(event) = maybe_agent_event {
                    maintainer.apply_agent_event(event);
                } else {
                    maintainer.poll_agent_task().await?;
                }
                needs_redraw = true;
            }
            maybe_event = events.next() => {
                let (app, agent_slot) = maintainer.split_mut();
                match maybe_event {
                    Some(Ok(event)) => match translate_event(event, app) {
                        Some(UiEvent::App(event)) => {
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
        maintainer.needs_redraw = needs_redraw;
    };
    if let Some(handle) = maintainer.app_mut().repo_context_task.take() {
        handle.abort();
    }
    teardown_terminal(terminal)?;
    if result.is_ok() {
        let _ = crate::auto_memory::drain_auto_memory_for_shutdown().await;
    }
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

fn should_start_initial_rebuild(
    initialize_local_embeddings: bool,
    explicit_plugin_dirs: &[PathBuf],
) -> bool {
    initialize_local_embeddings || !explicit_plugin_dirs.is_empty()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::should_start_initial_rebuild;

    #[test]
    fn initial_rebuild_starts_for_embeddings_or_explicit_plugin_dirs() {
        assert!(should_start_initial_rebuild(true, &[]));
        assert!(should_start_initial_rebuild(
            false,
            &[PathBuf::from("/plugins")]
        ));
        assert!(!should_start_initial_rebuild(false, &[]));
    }
}
