use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::oneshot;

use super::TuiApp;
use crate::agent::Agent;
use crate::context::RuntimeContextFiles;
use crate::prompt::{PromptMode, PromptRuntimeConfig};
use crate::workspace::WorkspaceMemory;

const REFRESH_INTERVAL: Duration = Duration::from_secs(2);

struct ContextRequest {
    workspace: Arc<WorkspaceMemory>,
    config: PromptRuntimeConfig,
    mode: PromptMode,
    session_id: String,
}

#[derive(Default)]
pub(super) struct ContextFileCache {
    request: Option<ContextRequest>,
    generation: u64,
    files: Option<RuntimeContextFiles>,
    loaded: bool,
    last_poll: Option<Instant>,
    pending: Option<(u64, oneshot::Receiver<RuntimeContextFiles>)>,
    error: Option<String>,
}

impl TuiApp {
    pub(super) fn observe_context_files(&mut self, agent: &Agent) -> &RuntimeContextFiles {
        let cache = &mut self.context_files;
        let changed = cache.request.as_ref().is_none_or(|request| {
            request.workspace.root != agent.workspace.root
                || request.workspace.rara_dir != agent.workspace.rara_dir
                || request.config != *agent.prompt_config()
                || request.mode != agent.prompt_mode()
                || request.session_id != agent.session_id
        });
        if changed {
            cache.generation = cache.generation.wrapping_add(1);
            cache.files = Some(RuntimeContextFiles::pending(
                &agent.workspace,
                agent.prompt_config(),
            ));
            cache.request = Some(ContextRequest {
                workspace: agent.workspace.clone(),
                config: agent.prompt_config().clone(),
                mode: agent.prompt_mode(),
                session_id: agent.session_id.clone(),
            });
            cache.loaded = false;
            cache.last_poll = None;
            cache.error = None;
        }
        cache.files.get_or_insert_with(|| {
            RuntimeContextFiles::pending(&agent.workspace, agent.prompt_config())
        })
    }

    pub(crate) fn context_files_status(&self) -> Option<&str> {
        if let Some(error) = self.context_files.error.as_deref() {
            Some(error)
        } else if self.context_files.request.is_some() && !self.context_files.loaded {
            Some("Loading workspace context...")
        } else {
            None
        }
    }

    pub(crate) fn poll_context_files(&mut self) -> bool {
        let cache = &mut self.context_files;
        let mut changed = false;
        if let Some((generation, receiver)) = cache.pending.as_mut() {
            match receiver.try_recv() {
                Ok(files) => {
                    if *generation == cache.generation {
                        cache.files = Some(files);
                        cache.loaded = true;
                        cache.error = None;
                        cache.last_poll = Some(Instant::now());
                        changed = true;
                    }
                    cache.pending = None;
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
                Err(oneshot::error::TryRecvError::Closed) => {
                    if *generation == cache.generation {
                        let message =
                            "Workspace context refresh failed; retrying in the background.";
                        log::warn!("{message}");
                        cache.error = Some(message.into());
                        cache.last_poll = Some(Instant::now());
                        changed = true;
                    }
                    cache.pending = None;
                }
            }
        }
        if cache.pending.is_some()
            || cache
                .last_poll
                .is_some_and(|last| last.elapsed() < REFRESH_INTERVAL)
        {
            return changed;
        }
        let Some(request) = &cache.request else {
            return changed;
        };
        let workspace = request.workspace.clone();
        let config = request.config.clone();
        let mode = request.mode;
        let (sender, receiver) = oneshot::channel();
        cache.pending = Some((cache.generation, receiver));
        tokio::task::spawn_blocking(move || {
            let files = RuntimeContextFiles::load(&workspace, &config, mode);
            // Display-only data has no recovery obligation after the view is gone.
            drop(sender.send(files));
        });
        changed
    }

    #[cfg(test)]
    pub(crate) async fn finish_context_files_for_test(&mut self, agent: &Agent) {
        if self.context_files.pending.is_none() {
            self.poll_context_files();
        }
        let (generation, receiver) = self
            .context_files
            .pending
            .take()
            .expect("pending context read");
        let files = tokio::time::timeout(Duration::from_secs(5), receiver)
            .await
            .expect("context read timeout")
            .expect("context worker");
        let (sender, receiver) = oneshot::channel();
        assert!(sender.send(files).is_ok());
        self.context_files.pending = Some((generation, receiver));
        self.poll_context_files();
        self.apply_runtime_snapshot(
            agent,
            crate::runtime_client::RuntimeClient::extension_snapshot_for_agent(agent, 0),
        );
    }
}

#[cfg(test)]
mod tests {
    use rara_memory::memory_handle::MemoryHandle;
    use rara_tools::tool::ToolManager;

    use super::*;
    use crate::config::ConfigManager;
    use crate::llm::MockLlm;
    use crate::session::SessionManager;
    use crate::tui::state::RuntimeExtensionSnapshot;

    #[tokio::test]
    async fn old_context_reply_cannot_complete_a_new_session_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let mut agent = Agent::new(
            ToolManager::new(),
            Arc::new(MockLlm),
            Arc::new(MemoryHandle::new(
                &data.join("memory").display().to_string(),
            )),
            Arc::new(SessionManager::new_for_rara_dir(data.clone()).unwrap()),
            Arc::new(WorkspaceMemory::from_paths(dir.path().to_path_buf(), data)),
        );
        let mut app = TuiApp::new(ConfigManager {
            path: dir.path().join("config.json"),
        })
        .unwrap();
        app.apply_runtime_snapshot(&agent, RuntimeExtensionSnapshot::default());
        let files = RuntimeContextFiles::pending(&agent.workspace, agent.prompt_config());
        let (sender, receiver) = oneshot::channel();
        app.context_files.pending = Some((app.context_files.generation, receiver));
        agent.set_session_id("replacement-session".into());
        app.apply_runtime_snapshot(&agent, RuntimeExtensionSnapshot::default());
        assert!(sender.send(files).is_ok());
        assert!(
            !app.poll_context_files(),
            "stale results cannot publish display data"
        );
        assert_eq!(
            app.context_files_status(),
            Some("Loading workspace context...")
        );
        assert_eq!(app.snapshot.session_id, "replacement-session");
        app.finish_context_files_for_test(&agent).await;
        assert_eq!(app.context_files_status(), None);
        assert_eq!(
            app.snapshot.prompt_source_entries,
            agent.shared_runtime_context().prompt.source_entries
        );
    }
}
