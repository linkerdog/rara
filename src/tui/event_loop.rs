use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;

use crossterm::{
    event::{Event, EventStream},
    terminal::size as terminal_size,
};
use futures::StreamExt;
use rara_state::state_db::StateDb;
use ratatui::backend::{Backend, CrosstermBackend};
use tokio::time::{Duration, Instant, MissedTickBehavior, interval};

use super::controller::{RuntimeActivity, TuiController};
use super::custom_terminal::Terminal;
use super::event_stream::{UiEvent, translate_event};
use super::external_editor::{EditorCommand, EditorDraft, EditorRequest, PreparedEdit};
use super::frame_scheduler::FrameScheduler;
use super::render::render;
use super::runtime::RuntimeCommandProcessor;
use super::runtime_port::{
    InProcessRuntimeClientPort, RuntimeClientPort, RuntimeCommand, RuntimeMaintenanceCommand,
};
use super::session_restore::apply_startup_resume;
use super::state::ListPickerKind;
use super::state::Overlay;
use super::state::TuiApp;
use super::submit::clamp_command_palette_selection;
use super::terminal_control::TerminalTarget;
use super::terminal_feedback::{TerminalFeedback, TitleMode};
use super::terminal_modes::TerminalModeGuard;
use super::terminal_ui::handle_paste;
use crate::oauth::OAuthManager;
use crate::runtime_client::RuntimeClient;
use crate::tui::message_role::MessageRole;
use crate::tui::state::NoticeLevel;

#[derive(Debug, Clone)]
pub enum StartupResumeTarget {
    Fresh,
    Latest,
    ThreadId(String),
    Picker,
}

pub struct TuiStartupOptions {
    pub config: crate::config::RaraConfig,
    pub resume: StartupResumeTarget,
    pub permission_override: Option<super::state::PermissionMode>,
}

pub async fn run_tui(
    runtime: RuntimeClient,
    oauth_manager: OAuthManager,
    startup: TuiStartupOptions,
) -> anyhow::Result<Option<String>> {
    let diagnostics = crate::diagnostics::TerminalDiagnostics::start()?;
    let mut terminal_modes =
        TerminalModeGuard::start(TitleMode::configured(&startup.config.tui.terminal))?;
    let result = TerminalModeGuard::run_owner(run_tui_session(
        runtime,
        oauth_manager,
        startup,
        &mut terminal_modes,
        diagnostics.reader(),
    ))
    .await?;
    if let Err(error) = terminal_modes.restore() {
        if result.is_ok() {
            return Err(error.into());
        }
        log::warn!("Failed to restore terminal after TUI error: {error}");
    }
    let completed = result?;
    completed.processor.drain_memory().await;
    completed.history_flush?;
    Ok(completed.session_id)
}

struct CompletedTuiSession {
    session_id: Option<String>,
    processor: RuntimeCommandProcessor,
    history_flush: anyhow::Result<()>,
}

async fn run_tui_session(
    runtime: RuntimeClient,
    oauth_manager: OAuthManager,
    startup: TuiStartupOptions,
    terminal_modes: &mut TerminalModeGuard,
    diagnostics: crate::diagnostics::DiagnosticReader,
) -> anyhow::Result<CompletedTuiSession> {
    let initial_size = terminal_size()?;
    let mut app = TuiApp::with_config(crate::config::ConfigManager::new()?, startup.config)?;
    app.attach_prompt_history();
    app.terminal_capabilities = rara_terminal_detection::TerminalCapabilities::detect();
    app.diagnostics = Some(diagnostics);
    app.goal_handle = runtime.goal_handle.clone();
    app.goal = runtime.goal_handle.snapshot();
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
            runtime
                .agent()
                .ok_or_else(|| anyhow::anyhow!("runtime agent is not ready for the TUI"))?
                .memory_store
                .clone(),
        ),
    ));
    app.sandbox_network_access
        .store(false, std::sync::atomic::Ordering::Relaxed);
    app.terminal_width = initial_size.0;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut processor = RuntimeCommandProcessor::new(runtime);
    let (runtime_port, runtime_commands) = InProcessRuntimeClientPort::new(
        processor.event_bus(),
        Arc::new(std::sync::RwLock::new(app.snapshot.clone().into_inner())),
    );
    let runtime_port: Arc<dyn RuntimeClientPort> = Arc::new(runtime_port);
    let startup_plugin_dirs = app.explicit_plugin_dirs.clone();
    let mut maintainer = TuiController::new(app, runtime_port, runtime_commands);
    match tokio::task::spawn_blocking(StateDb::new).await? {
        Ok(state_db) => {
            let state_db = Arc::new(state_db);
            let app = maintainer.app_mut();
            let agent_slot = processor.agent_mut();
            app.attach_state_db(state_db);
            apply_startup_resume(&startup.resume, app, agent_slot);
        }
        Err(err) => maintainer.app_mut().set_state_db_error(err.to_string()),
    }
    let result = async {
        if let Some(mode) = startup.permission_override {
            processor
                .apply_command(
                    maintainer.app_mut(),
                    RuntimeCommand::SetPermissionMode(mode),
                )
                .await?;
        }
        let oauth_manager = Arc::new(oauth_manager);
        maintainer.app_mut().codex_auth_mode = oauth_manager.saved_auth_mode().ok().flatten();

        maintainer.sync_snapshot(&mut processor).await?;
        maintainer.start_repo_context_detection();

        {
            let mut events = TerminalEventSource::new(terminal_modes);
            run_event_loop(
                &mut terminal,
                &mut maintainer,
                &mut processor,
                &oauth_manager,
                &mut events,
                if should_start_initial_rebuild(&startup_plugin_dirs) {
                    StartupMaintenance::Rebuild
                } else {
                    StartupMaintenance::None
                },
            )
            .await
        }
    }
    .await;
    let storage_result = maintainer.app_mut().shutdown_storage().await;
    if let Err(error) = &storage_result {
        log::warn!("Storage cleanup failed; writes were not acknowledged: {error:#}");
    }
    if let Some(handle) = maintainer.app_mut().repo_context_task.take() {
        handle.abort();
    }
    let handoff = terminal
        .finish_inline_viewport()
        .map_err(anyhow::Error::from);
    if let Err(error) = &handoff {
        log::warn!("Failed to position shell cursor after TUI error: {error}");
    }
    let restored = terminal_modes.restore().map_err(anyhow::Error::from);
    if let Err(error) = &restored {
        log::warn!("Failed to restore terminal before history cleanup: {error}");
    }
    let history = maintainer.app_mut().shutdown_prompt_history().await;
    if let Err(error) = &history {
        log::warn!("Prompt history cleanup failed: {error:#}");
    }
    result.and(storage_result).and(handoff).and(restored)?;
    let session_id = processor.session_id().or_else(|| {
        (!maintainer.app().snapshot.session_id.is_empty())
            .then(|| maintainer.app().snapshot.session_id.clone())
    });
    Ok(CompletedTuiSession {
        session_id,
        processor,
        history_flush: history,
    })
}

/// Owns terminal input and mode handoff. Implementors must release the input
/// reader before terminal handoff and surface maintenance/reacquisition errors.
/// Input reads must be cancellation-safe because select drops losing futures.
pub(super) trait EventSource<B: Backend<Error = io::Error> + Write> {
    async fn next_event(&mut self) -> Option<io::Result<Event>>;
    fn maintain_raw_mode(&mut self) -> io::Result<()>;
    /// Suspend presentation/input for an editor; outer errors mean terminal ownership was lost.
    async fn edit_external(
        &mut self,
        terminal: &mut Terminal<B>,
        request: EditorRequest,
    ) -> io::Result<anyhow::Result<String>>;
    #[cfg(unix)]
    fn suspend(&mut self, terminal: &mut Terminal<B>) -> io::Result<()>;
}

pub(super) struct TerminalEventSource<'a> {
    events: Option<EventStream>,
    modes: &'a mut TerminalModeGuard,
}

impl<'a> TerminalEventSource<'a> {
    pub(super) fn new(modes: &'a mut TerminalModeGuard) -> Self {
        Self {
            events: Some(EventStream::new()),
            modes,
        }
    }
}

impl EventSource<CrosstermBackend<io::Stdout>> for TerminalEventSource<'_> {
    async fn next_event(&mut self) -> Option<io::Result<Event>> {
        self.events.as_mut()?.next().await
    }

    fn maintain_raw_mode(&mut self) -> io::Result<()> {
        self.modes.maintain_raw_mode()
    }

    async fn edit_external(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
        request: EditorRequest,
    ) -> io::Result<anyhow::Result<String>> {
        let command = match EditorCommand::from_environment() {
            Ok(command) => command,
            Err(error) => return Ok(Err(error)),
        };
        let edit = match PreparedEdit::prepare(request, command).await {
            Ok(edit) => edit,
            Err(error) => return Ok(Err(error)),
        };
        let events = self
            .events
            .take()
            .ok_or_else(|| io::Error::other("terminal reader unavailable for editor"))?;
        let (events, result) =
            super::external_editor::edit_with_terminal(terminal, self.modes, events, edit).await?;
        self.events = Some(events);
        Ok(result)
    }

    #[cfg(unix)]
    fn suspend(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
        let events = self
            .events
            .take()
            .ok_or_else(|| io::Error::other("terminal reader unavailable for suspend"))?;
        self.events = Some(super::job_control::suspend(terminal, self.modes, events)?);
        Ok(())
    }
}

pub(super) enum StartupMaintenance {
    None,
    Rebuild,
}

// Keep the terminal alive across loop errors so shell handoff precedes mode restoration.
async fn run_event_loop<B: Backend<Error = io::Error> + Write>(
    terminal: &mut Terminal<B>,
    maintainer: &mut TuiController,
    processor: &mut RuntimeCommandProcessor,
    oauth_manager: &Arc<OAuthManager>,
    events: &mut impl EventSource<B>,
    mut startup_maintenance: StartupMaintenance,
) -> anyhow::Result<()> {
    let mut tick = interval(Duration::from_millis(166));
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut frames = FrameScheduler::default();
    let mut exit_flush: Option<tokio::sync::oneshot::Receiver<anyhow::Result<()>>> = None;
    let mut input_closed = false;
    let mut feedback = TerminalFeedback::new(
        TitleMode::configured(&maintainer.app().config.tui.terminal),
        TerminalTarget::from_environment(),
    );

    loop {
        let mut needs_redraw = std::mem::take(&mut maintainer.needs_redraw);
        needs_redraw |= maintainer.app_mut().poll_prompt_history();
        needs_redraw |= maintainer.app_mut().poll_file_mentions();
        needs_redraw |= maintainer.app_mut().poll_diagnostics();
        if exit_flush.is_none() {
            if matches!(startup_maintenance, StartupMaintenance::Rebuild)
                && !maintainer.app().is_busy()
            {
                startup_maintenance = StartupMaintenance::None;
                maintainer
                    .app_mut()
                    .push_entry(MessageRole::Runtime, "Loading explicit plugin directories.");
                maintainer
                    .send_runtime_command(RuntimeCommand::Maintenance(
                        RuntimeMaintenanceCommand::Rebuild,
                    ))
                    .await?;
            }
            needs_redraw |= maintainer.queue_restored_goal(processor).await;
            if maintainer.poll_repo_context().await {
                needs_redraw = true;
            }
            needs_redraw |= maintainer.app_mut().check_composer_paste_flush();
        }
        feedback.update(maintainer.app_mut(), terminal.control_writer())?;
        if needs_redraw {
            frames.request(Instant::now());
        }
        if frames.is_due(Instant::now()) {
            let app = maintainer.app_mut();
            clamp_command_palette_selection(app);
            app.terminal_width = terminal.size()?.width;
            terminal.draw_inline(|f| render(f, app))?;
            frames.mark_drawn(Instant::now());
        }
        needs_redraw = false;

        tokio::select! {
            result = async {
                match exit_flush.as_mut() {
                    Some(receiver) => receiver.await,
                    None => futures::future::pending().await,
                }
            } => {
                exit_flush = None;
                let result = result.unwrap_or_else(|_| Err(anyhow::anyhow!("storage worker stopped before exit acknowledgement")));
                match result {
                    Ok(()) => {
                        if let Some(task) = maintainer.app_mut().bottom_pane.running_task.take() {
                            task.handle.abort();
                        }
                        break;
                    }
                    Err(error) => {
                        if input_closed {
                            return Err(error);
                        }
                        let app = maintainer.app_mut();
                        app.poll_storage();
                        app.push_unpersisted_notice(NoticeLevel::Error, format!("Could not save session; exit cancelled: {error:#}"));
                        needs_redraw = true;
                    }
                }
            }
            _ = frames.wait() => {}
            _ = tick.tick() => {
                events.maintain_raw_mode()?;
                if exit_flush.is_some() {
                    maintainer.needs_redraw = true;
                    continue;
                }
                let mut changed = false;
                let app = maintainer.app_mut();
                if let Some(clipboard) = &mut app.clipboard
                    && let Some(notice) = clipboard.poll().await
                {
                    app.push_notice(notice.level, notice.message);
                    changed = true;
                }
                changed |= app.quit_shortcut.expire(std::time::Instant::now());
                changed |= app.expire_notice(Instant::now());
                if let Some(delta) = app.transcript_selection.autoscroll_delta() {
                    super::render::scroll_transcript(app, delta);
                    changed = true;
                }
                changed |= super::goal_ui::update_elapsed(app, crate::runtime_goals::current_unix_timestamp_secs());
                changed |= app.poll_shared_task_files();
                changed |= app.poll_storage();
                changed |= super::diff_view::poll(app).await;
                changed |= app.poll_resume_queries();
                changed |= super::session_restore::poll_restore(app, processor.agent_mut());
                if app.poll_context_files() {
                    processor.sync_snapshot(app);
                    changed = true;
                }
                changed |= processor.sync_agent_activity(app);
                changed |= super::runtime::emit_query_heartbeat(app);
                needs_redraw |= changed;
            }
            runtime_activity = maintainer.wait_for_runtime_activity(), if exit_flush.is_none() => {
                needs_redraw |= apply_runtime_activity(maintainer, processor, runtime_activity).await?;
            }
            maybe_event = events.next_event(), if !input_closed => {
                if exit_flush.is_some() {
                    match maybe_event {
                        Some(Ok(Event::Key(key))) if key.code == crossterm::event::KeyCode::Esc => {
                            exit_flush = None;
                            maintainer.app_mut().push_notice(NoticeLevel::Info, "Exit cancelled; pending writes remain queued.");
                        }
                        Some(Ok(Event::Resize(_, _))) => terminal.invalidate_viewport(),
                        Some(Err(error)) => log::warn!("Terminal event error while saving: {error}"),
                        None => input_closed = true,
                        Some(Ok(_)) => {}
                    }
                    maintainer.needs_redraw = true;
                    continue;
                }
                match maybe_event {
                    Some(Ok(event)) => match translate_event(event, maintainer.app_mut()) {
                        Some(UiEvent::App(event)) => {
                            if maintainer
                                .dispatch_event(processor, event, oauth_manager)
                                .await?
                            {
                                if maintainer.app().is_busy() {
                                    super::goal_resume::defer_for_user_stop(maintainer.app_mut());
                                }
                                let app = maintainer.app_mut();
                                app.pending_restore = None;
                                app.finalize_active_turn();
                                app.persist_runtime_state();
                                match app.storage_flush_receiver() {
                                    Ok(receiver) => {
                                        exit_flush = Some(receiver);
                                        app.push_unpersisted_notice(NoticeLevel::Info, "Saving session before exit... Press Esc to stay.");
                                    }
                                    Err(error) => app.push_unpersisted_notice(NoticeLevel::Error, format!("Could not save session; exit cancelled: {error:#}")),
                                }
                            }
                            needs_redraw = true;
                        }
                        Some(UiEvent::ExternalEditor) => {
                            let (draft, request) = EditorDraft::capture(maintainer.app_mut());
                            let result = {
                                let edit = events.edit_external(terminal, request);
                                tokio::pin!(edit);
                                loop {
                                    tokio::select! {
                                        result = &mut edit => break result?,
                                        activity = maintainer.wait_for_runtime_activity() => {
                                            apply_runtime_activity(maintainer, processor, activity).await?;
                                        }
                                    }
                                }
                            };
                            draft.finish(maintainer.app_mut(), result).await;
                            feedback.resumed();
                            needs_redraw = true;
                        }
                        Some(UiEvent::Draw) => {
                            terminal.invalidate_viewport();
                            needs_redraw = true;
                        }
                        Some(UiEvent::Paste(text)) => {
                            let app = maintainer.app_mut();
                            handle_paste(text, app);
                            needs_redraw = true;
                        }
                        Some(UiEvent::FocusChanged(_focused)) => {
                            maintainer.sync_snapshot(processor).await?;
                            maintainer.publish_snapshot_projection();
                            needs_redraw = true;
                        }
                        #[cfg(unix)]
                        Some(UiEvent::Suspend) => {
                            events.suspend(terminal)?;
                            feedback.resumed();
                            needs_redraw = true;
                        }
                        None => {}
                    },
                    Some(Err(err)) => {
                        maintainer
                            .app_mut()
                            .push_notice(NoticeLevel::Error, format!("Terminal event error: {err}"));
                        needs_redraw = true;
                    }
                    None => {
                        let app = maintainer.app_mut();
                        app.pending_restore = None;
                        app.finalize_active_turn();
                        app.persist_runtime_state();
                        break;
                    }
                }
            }
        }
        maintainer.needs_redraw |= needs_redraw;
    }
    Ok(())
}

async fn apply_runtime_activity(
    maintainer: &mut TuiController,
    processor: &mut RuntimeCommandProcessor,
    activity: RuntimeActivity,
) -> anyhow::Result<bool> {
    let mut changed = false;
    match activity {
        RuntimeActivity::Event(Some(event)) => {
            changed |= maintainer.apply_runtime_event(event);
            changed |= maintainer.complete_query_if_ready(processor).await?;
        }
        RuntimeActivity::Event(None) => {}
        RuntimeActivity::Completed(completion) => {
            changed |= maintainer
                .receive_runtime_task_completion(processor, completion)
                .await?;
        }
        RuntimeActivity::Command(Some(command)) => {
            maintainer.apply_runtime_command(processor, command).await?;
            changed = true;
        }
        RuntimeActivity::Command(None) => {}
    }
    changed |= maintainer.resync_after_event_loss(processor);
    Ok(changed)
}

fn should_start_initial_rebuild(explicit_plugin_dirs: &[PathBuf]) -> bool {
    !explicit_plugin_dirs.is_empty()
}

#[cfg(test)]
#[path = "event_loop_tests.rs"]
mod loop_tests;

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
