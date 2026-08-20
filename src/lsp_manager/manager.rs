use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::Mutex as AsyncMutex;

use super::cache::DiagnosticCache;
use super::runtime::{ServerRuntime, spawn_server};
use super::types::{
    LspDiagnostics, LspFailure, LspFailureKind, LspServerPhase, LspServerStatus, LspStatusSnapshot,
    ServerKind, all_server_kinds,
};

#[derive(Clone)]
pub(super) struct LspManagerOptions {
    commands: HashMap<ServerKind, Vec<OsString>>,
    initialize_timeout: Duration,
    retry_backoff: Duration,
    max_start_attempts: usize,
}

impl Default for LspManagerOptions {
    fn default() -> Self {
        Self {
            commands: all_server_kinds()
                .into_iter()
                .map(|kind| (kind, kind.command()))
                .collect(),
            initialize_timeout: Duration::from_secs(45),
            retry_backoff: Duration::from_secs(2),
            max_start_attempts: 3,
        }
    }
}

impl LspManagerOptions {
    #[cfg(test)]
    pub(super) fn with_command(mut self, kind: ServerKind, command: Vec<OsString>) -> Self {
        self.commands.insert(kind, command);
        self
    }

    #[cfg(test)]
    pub(super) fn with_initialize_timeout(mut self, timeout: Duration) -> Self {
        self.initialize_timeout = timeout;
        self
    }

    #[cfg(test)]
    pub(super) fn with_retry_backoff(mut self, retry_backoff: Duration) -> Self {
        self.retry_backoff = retry_backoff;
        self
    }
}

pub struct LspManager {
    slots: HashMap<ServerKind, Arc<ServerSlot>>,
    diagnostics: Arc<Mutex<DiagnosticCache>>,
    last_failure: Arc<Mutex<Option<LspFailure>>>,
    workspace_root: PathBuf,
    enabled: bool,
    options: LspManagerOptions,
}

struct ServerSlot {
    detected: bool,
    state: Mutex<ServerState>,
    start_gate: AsyncMutex<()>,
    attempts: AtomicUsize,
}

#[derive(Clone)]
enum ServerState {
    NotStarted,
    Starting {
        generation: usize,
    },
    Ready {
        generation: usize,
        runtime: Arc<ServerRuntime>,
    },
    Unavailable {
        failure: LspFailure,
    },
    Failed {
        failure: LspFailure,
        failed_at: Instant,
    },
}

struct StartupAttemptGuard {
    slot: Arc<ServerSlot>,
    kind: ServerKind,
    generation: usize,
    last_failure: Arc<Mutex<Option<LspFailure>>>,
    completed: bool,
}

impl StartupAttemptGuard {
    fn complete(&mut self) {
        self.completed = true;
    }
}

impl Drop for StartupAttemptGuard {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        let failure = LspFailure::new(
            LspFailureKind::ProtocolError,
            format!(
                "{} startup was cancelled before completion",
                self.kind.label()
            ),
            true,
        )
        .for_server(self.kind);
        let mut state = self.slot.state.lock().unwrap();
        if matches!(
            &*state,
            ServerState::Starting {
                generation: active_generation,
            } if *active_generation == self.generation
        ) {
            *state = ServerState::Failed {
                failure: failure.clone(),
                failed_at: Instant::now(),
            };
            *self.last_failure.lock().unwrap() = Some(failure);
        }
    }
}

impl LspManager {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self::new_with_options(
            workspace_root,
            lsp_enabled_from_env(),
            LspManagerOptions::default(),
        )
    }

    fn new_with_options(
        workspace_root: PathBuf,
        enabled: bool,
        options: LspManagerOptions,
    ) -> Self {
        let slots = all_server_kinds()
            .into_iter()
            .map(|kind| {
                (
                    kind,
                    Arc::new(ServerSlot {
                        detected: workspace_root.join(kind.detect_file()).exists(),
                        state: Mutex::new(ServerState::NotStarted),
                        start_gate: AsyncMutex::new(()),
                        attempts: AtomicUsize::new(0),
                    }),
                )
            })
            .collect();
        Self {
            slots,
            diagnostics: Arc::new(Mutex::new(DiagnosticCache::default())),
            last_failure: Arc::new(Mutex::new(None)),
            workspace_root,
            enabled,
            options,
        }
    }

    #[cfg(test)]
    pub(super) fn for_test(workspace_root: PathBuf, options: LspManagerOptions) -> Self {
        Self::new_with_options(workspace_root, true, options)
    }

    pub async fn diagnostics_for(&self, file_path: &Path) -> Result<LspDiagnostics, LspFailure> {
        if !self.enabled {
            return Err(self.record_failure(LspFailure::new(
                LspFailureKind::Disabled,
                "LSP is disabled by RARA_LSP",
                false,
            )));
        }
        let file_path = self.resolve_file_path(file_path);
        let kind = self.detect_server(&file_path)?;
        let runtime = self.ensure_server_ready(kind).await?;
        if let Err(failure) = runtime.sync_file(&file_path, &self.diagnostics).await {
            return Err(self.record_failure(failure));
        }
        Ok(self.diagnostics.lock().unwrap().get(&file_path))
    }

    pub fn diagnostics_summary(&self) -> String {
        self.diagnostics.lock().unwrap().summary()
    }

    pub fn status_snapshot(&self) -> LspStatusSnapshot {
        let servers = all_server_kinds()
            .into_iter()
            .map(|kind| self.server_status(kind))
            .collect();
        let (diagnostic_file_count, diagnostic_count) = self.diagnostics.lock().unwrap().counts();
        let last_failure = self.last_failure.lock().unwrap().clone();
        LspStatusSnapshot {
            enabled: self.enabled,
            servers,
            diagnostic_file_count,
            diagnostic_count,
            last_error: last_failure.as_ref().map(ToString::to_string),
            last_failure,
        }
    }

    async fn ensure_server_ready(
        &self,
        kind: ServerKind,
    ) -> Result<Arc<ServerRuntime>, LspFailure> {
        let slot = self.slots.get(&kind).expect("all server slots are present");
        let _start_guard = slot.start_gate.lock().await;
        match slot.state.lock().unwrap().clone() {
            ServerState::Ready { runtime, .. } if runtime.is_running() => return Ok(runtime),
            ServerState::Unavailable { failure } => return Err(failure),
            ServerState::Failed { failure, failed_at }
                if !failure.retryable
                    || slot.attempts.load(Ordering::Relaxed) >= self.options.max_start_attempts
                    || failed_at.elapsed() < self.options.retry_backoff =>
            {
                return Err(failure);
            }
            ServerState::Starting { generation } => {
                let failure = LspFailure::new(
                    LspFailureKind::ProtocolError,
                    format!(
                        "{} startup generation {generation} was cancelled before completion",
                        kind.label()
                    ),
                    true,
                )
                .for_server(kind);
                if slot.attempts.load(Ordering::Relaxed) >= self.options.max_start_attempts {
                    self.store_start_failure(slot, failure.clone());
                    return Err(failure);
                }
                log::warn!("{failure}; retrying");
                *self.last_failure.lock().unwrap() = Some(failure);
            }
            ServerState::NotStarted | ServerState::Ready { .. } | ServerState::Failed { .. } => {}
        }

        let generation = slot.attempts.fetch_add(1, Ordering::Relaxed) + 1;
        *slot.state.lock().unwrap() = ServerState::Starting { generation };
        let mut startup_guard = StartupAttemptGuard {
            slot: slot.clone(),
            kind,
            generation,
            last_failure: self.last_failure.clone(),
            completed: false,
        };
        let command = self
            .options
            .commands
            .get(&kind)
            .cloned()
            .unwrap_or_else(|| kind.command());
        match spawn_server(
            kind,
            &command,
            &self.workspace_root,
            self.diagnostics.clone(),
            self.options.initialize_timeout,
        )
        .await
        {
            Ok(runtime) => {
                if !runtime.is_running() {
                    let failure = runtime.observed_exit().unwrap_or_else(|| {
                        LspFailure::new(
                            LspFailureKind::ServerExited,
                            format!("{} exited during startup", kind.label()),
                            true,
                        )
                        .for_server(kind)
                    });
                    self.store_start_failure(slot, failure.clone());
                    startup_guard.complete();
                    return Err(failure);
                }
                *slot.state.lock().unwrap() = ServerState::Ready {
                    generation,
                    runtime: runtime.clone(),
                };
                startup_guard.complete();
                *self.last_failure.lock().unwrap() = None;
                watch_server_exit(
                    slot.clone(),
                    generation,
                    runtime.exit_receiver(),
                    self.last_failure.clone(),
                );
                Ok(runtime)
            }
            Err(failure) => {
                self.store_start_failure(slot, failure.clone());
                startup_guard.complete();
                Err(failure)
            }
        }
    }

    fn store_start_failure(&self, slot: &ServerSlot, failure: LspFailure) {
        let state = if failure.kind == LspFailureKind::BinaryMissing {
            ServerState::Unavailable {
                failure: failure.clone(),
            }
        } else {
            ServerState::Failed {
                failure: failure.clone(),
                failed_at: Instant::now(),
            }
        };
        *slot.state.lock().unwrap() = state;
        *self.last_failure.lock().unwrap() = Some(failure);
    }

    fn detect_server(&self, file_path: &Path) -> Result<ServerKind, LspFailure> {
        let extension = file_path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| format!(".{extension}"))
            .unwrap_or_default();
        for kind in all_server_kinds() {
            if kind.extensions().contains(&extension.as_str())
                && self.slots.get(&kind).is_some_and(|slot| slot.detected)
            {
                return Ok(kind);
            }
        }
        Err(self.record_failure(LspFailure::new(
            LspFailureKind::UnsupportedFile,
            format!("no detected LSP server for {}", file_path.display()),
            false,
        )))
    }

    fn server_status(&self, kind: ServerKind) -> LspServerStatus {
        let slot = self.slots.get(&kind).expect("all server slots are present");
        let (phase, checked, available, running, last_failure) =
            match slot.state.lock().unwrap().clone() {
                ServerState::NotStarted => (LspServerPhase::NotStarted, false, false, false, None),
                ServerState::Starting { .. } => (LspServerPhase::Starting, true, true, false, None),
                ServerState::Ready { runtime, .. } => (
                    LspServerPhase::Ready,
                    true,
                    true,
                    runtime.is_running(),
                    None,
                ),
                ServerState::Unavailable { failure } => (
                    LspServerPhase::Unavailable,
                    true,
                    false,
                    false,
                    Some(failure),
                ),
                ServerState::Failed { failure, .. } => {
                    (LspServerPhase::Failed, true, true, false, Some(failure))
                }
            };
        LspServerStatus {
            name: kind.label().to_string(),
            detected: slot.detected,
            checked,
            available: slot.detected && available,
            running,
            phase,
            last_failure,
        }
    }

    fn record_failure(&self, failure: LspFailure) -> LspFailure {
        *self.last_failure.lock().unwrap() = Some(failure.clone());
        failure
    }

    fn resolve_file_path(&self, file_path: &Path) -> PathBuf {
        if file_path.is_absolute() {
            file_path.to_path_buf()
        } else {
            self.workspace_root.join(file_path)
        }
    }

    #[cfg(test)]
    pub(super) fn start_attempts(&self, kind: ServerKind) -> usize {
        self.slots
            .get(&kind)
            .expect("server slot")
            .attempts
            .load(Ordering::Relaxed)
    }
}

fn watch_server_exit(
    slot: Arc<ServerSlot>,
    generation: usize,
    mut exit_rx: tokio::sync::watch::Receiver<Option<LspFailure>>,
    last_failure: Arc<Mutex<Option<LspFailure>>>,
) {
    tokio::spawn(async move {
        let failure = if let Some(failure) = exit_rx.borrow().clone() {
            failure
        } else {
            if exit_rx.changed().await.is_err() {
                return;
            }
            let observed = exit_rx.borrow().clone();
            let Some(failure) = observed else {
                return;
            };
            failure
        };
        let mut state = slot.state.lock().unwrap();
        if matches!(
            &*state,
            ServerState::Ready {
                generation: active_generation,
                ..
            } if *active_generation == generation
        ) {
            *state = ServerState::Failed {
                failure: failure.clone(),
                failed_at: Instant::now(),
            };
            *last_failure.lock().unwrap() = Some(failure);
        }
    });
}

fn lsp_enabled_from_env() -> bool {
    std::env::var("RARA_LSP")
        .map(|value| {
            let value = value.trim().to_ascii_lowercase();
            !(value == "0" || value == "false" || value == "off")
        })
        .unwrap_or(true)
}
