use std::collections::HashMap;
use std::env;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use rara_tools::tool::ToolError;
use uuid::Uuid;

use super::process::{kill_pty_child, restore_running_after_failed_kill};
use super::types::{PtySessionRecord, PtySessionSnapshot, PtySessionStatus};
use crate::sandbox::WrappedCommand;

pub(crate) const START_QUICK_COMPLETION_TIMEOUT: Duration = Duration::from_millis(750);
const PTY_START_QUICK_COMPLETION_POLL: Duration = Duration::from_millis(25);
const MAX_PTY_SESSIONS: usize = 15;
const PTY_IDLE_PRUNE_SECS: u64 = 120;

pub struct PtySessionStore {
    dir: PathBuf,
    pub(crate) sessions: Mutex<HashMap<String, PtySessionRecord>>,
}

impl PtySessionStore {
    pub fn new(dir: PathBuf) -> Result<Self, ToolError> {
        std::fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            sessions: Mutex::new(HashMap::new()),
        })
    }

    fn prune_idle_sessions(&self) -> usize {
        let sessions = self.sessions.lock().expect("pty session store lock");
        let count = sessions.len();
        if count < MAX_PTY_SESSIONS {
            return 0;
        }
        let now = Instant::now();
        let idle_cutoff = Duration::from_secs(PTY_IDLE_PRUNE_SECS);

        // Collect (id, last_read, has_exited) for all sessions
        let meta: Vec<(String, Instant, bool)> = sessions
            .iter()
            .map(|(id, s)| {
                let exited = !matches!(
                    *s.status.lock().expect("pty status lock"),
                    PtySessionStatus::Running
                );
                (id.clone(), s.last_read, exited)
            })
            .collect();

        // Protect the 8 most recently read sessions
        let mut by_recency = meta.clone();
        by_recency.sort_by_key(|(_, last, _)| std::cmp::Reverse(*last));
        let protected: std::collections::HashSet<&str> = by_recency
            .iter()
            .take(8)
            .map(|(id, _, _)| id.as_str())
            .collect();

        // Idle = not read recently
        let is_idle = |last: Instant| now.duration_since(last) >= idle_cutoff;

        // Phase 1: prefer idle + exited
        let mut by_lru = meta.clone();
        by_lru.sort_by_key(|(_, last, _)| *last);
        let excess = count.saturating_sub(MAX_PTY_SESSIONS - 1);

        let mut to_prune = Vec::with_capacity(excess);
        let mut idle_running_candidates = Vec::new();

        for (id, last, exited) in &by_lru {
            if !protected.contains(id.as_str()) && is_idle(*last) {
                if *exited {
                    to_prune.push(id.clone());
                } else {
                    idle_running_candidates.push(id.clone());
                }
            }
        }

        if to_prune.len() < excess {
            let needed = excess - to_prune.len();
            to_prune.extend(idle_running_candidates.into_iter().take(needed));
        } else {
            to_prune.truncate(excess);
        }

        let pruned = to_prune.len();
        if !to_prune.is_empty() {
            for id in &to_prune {
                if let Some(session) = sessions.get(id)
                    && let Ok(mut status) = session.status.lock()
                {
                    *status = PtySessionStatus::Killed;
                }
            }
        }
        pruned
    }

    #[allow(clippy::too_many_arguments)]
    // PTY start keeps the process launch parameters explicit at the spawn
    // boundary.
    pub(crate) fn start(
        &self,
        command: String,
        wrapped: WrappedCommand,
        cwd: String,
        base_env: &HashMap<String, String>,
        env: HashMap<String, String>,
        rows: u16,
        cols: u16,
    ) -> Result<PtySessionSnapshot, ToolError> {
        let id = format!("pty-{}", Uuid::new_v4());
        let output_path = self.dir.join(format!("{id}.log"));
        self.prune_idle_sessions();
        let pty_system = NativePtySystem::default();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|err| ToolError::ExecutionFailed(format!("open pty: {err}")))?;
        let command_env = command_env_for_wrapped(&wrapped, base_env, &env)?;
        if wrapped.sandboxed && wrapped.sandbox_backend == "macos-seatbelt" {
            let sandbox_home = wrapped.sandbox_home.as_deref().ok_or_else(|| {
                ToolError::ExecutionFailed("sandboxed pty is missing sandbox home".into())
            })?;
            ensure_sandbox_home_dirs(sandbox_home)?;
        }

        let mut cmd = CommandBuilder::new(&wrapped.program);
        for arg in &wrapped.args {
            cmd.arg(arg);
        }
        cmd.cwd(&cwd);
        if wrapped.sandboxed {
            cmd.env_clear();
        }
        for (key, value) in command_env {
            cmd.env(key, value);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|err| ToolError::ExecutionFailed(format!("spawn pty command: {err}")))?;
        let child_pid = child.process_id();
        drop(pair.slave);
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|err| ToolError::ExecutionFailed(format!("clone pty reader: {err}")))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|err| ToolError::ExecutionFailed(format!("take pty writer: {err}")))?;
        let child = Arc::new(Mutex::new(child));
        let status = Arc::new(Mutex::new(PtySessionStatus::Running));
        let reader_status = status.clone();
        let mut output_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&output_path)
            .map_err(|err| ToolError::ExecutionFailed(format!("open pty session log: {err}")))?;

        thread::spawn(move || {
            let mut buffer = [0_u8; 4096];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        let _ = output_file.write_all(&buffer[..n]);
                        let _ = output_file.flush();
                    }
                    Err(_) => break,
                }
            }
            let mut status = reader_status.lock().expect("pty status lock");
            match *status {
                PtySessionStatus::Running => *status = PtySessionStatus::Completed,
                PtySessionStatus::Killing => *status = PtySessionStatus::Killed,
                PtySessionStatus::Completed | PtySessionStatus::Killed => {}
            }
        });

        let record = PtySessionRecord {
            id: id.clone(),
            command,
            output_path,
            sandboxed: wrapped.sandboxed,
            sandbox_backend: wrapped.sandbox_backend,
            network_access: wrapped.network_access,
            writer: Arc::new(Mutex::new(writer)),
            child,
            child_pid,
            status,
            last_read: Instant::now(),
        };
        let snapshot = record.snapshot();
        self.sessions
            .lock()
            .expect("pty session store lock")
            .insert(id, record);
        Ok(snapshot)
    }

    pub(crate) fn get(&self, id: &str) -> Option<PtySessionSnapshot> {
        self.sessions
            .lock()
            .expect("pty session store lock")
            .get_mut(id)
            .map(|record| {
                record.last_read = Instant::now();
                record.snapshot()
            })
    }

    pub(crate) async fn wait_for_quick_completion(
        &self,
        id: &str,
        timeout: Duration,
    ) -> PtySessionSnapshot {
        let Some(deadline) = Instant::now().checked_add(timeout) else {
            // Timeout too large — return whatever snapshot we have now.
            return self
                .get(id)
                .unwrap_or_else(|| PtySessionSnapshot::missing(id));
        };

        // Fetch the session status once so polling avoids locking the session
        // store and cloning the full snapshot on every iteration.
        let status = {
            let sessions = self.sessions.lock().expect("pty session store lock");
            sessions.get(id).map(|record| record.status.clone())
        };
        let Some(status) = status else {
            return PtySessionSnapshot::missing(id);
        };

        while matches!(
            *status.lock().expect("pty status lock"),
            PtySessionStatus::Running
        ) {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let remaining = deadline - now;
            let sleep_duration = remaining.min(PTY_START_QUICK_COMPLETION_POLL);
            tokio::time::sleep(sleep_duration).await;
        }
        self.get(id)
            .unwrap_or_else(|| PtySessionSnapshot::missing(id))
    }

    pub(crate) fn list(&self) -> Vec<PtySessionSnapshot> {
        let mut snapshots = self
            .sessions
            .lock()
            .expect("pty session store lock")
            .values()
            .map(PtySessionRecord::snapshot)
            .collect::<Vec<_>>();
        snapshots.sort_by(|left, right| left.id.cmp(&right.id));
        snapshots
    }

    pub(crate) fn write(&self, id: &str, input: &str) -> Result<PtySessionSnapshot, ToolError> {
        let writer = {
            let sessions = self.sessions.lock().expect("pty session store lock");
            let record = sessions
                .get(id)
                .ok_or_else(|| ToolError::InvalidInput(format!("unknown pty session id: {id}")))?;
            record.writer.clone()
        };
        let mut writer = writer.lock().expect("pty writer lock");
        writer.write_all(input.as_bytes())?;
        writer.flush()?;
        drop(writer);
        self.get(id)
            .ok_or_else(|| ToolError::InvalidInput(format!("unknown pty session id: {id}")))
    }

    pub(crate) fn kill(&self, id: &str) -> Result<PtySessionSnapshot, ToolError> {
        let (child, child_pid, status, mut snapshot) = {
            let sessions = self.sessions.lock().expect("pty session store lock");
            let record = sessions
                .get(id)
                .ok_or_else(|| ToolError::InvalidInput(format!("unknown pty session id: {id}")))?;
            (
                record.child.clone(),
                record.child_pid,
                record.status.clone(),
                record.snapshot(),
            )
        };
        let should_kill = {
            let mut status = status.lock().expect("pty status lock");
            if matches!(*status, PtySessionStatus::Running) {
                *status = PtySessionStatus::Killing;
            }
            snapshot.status = *status;
            matches!(*status, PtySessionStatus::Killing)
        };
        if should_kill
            && let Err(err) =
                kill_pty_child(&mut **child.lock().expect("pty child lock"), child_pid)
        {
            restore_running_after_failed_kill(&status);
            return Err(ToolError::ExecutionFailed(format!(
                "kill pty session: {err}"
            )));
        }
        Ok(snapshot)
    }

    pub(crate) fn kill_all(&self) -> Vec<PtySessionSnapshot> {
        let ids = self
            .list()
            .into_iter()
            .filter(|snapshot| matches!(snapshot.status, PtySessionStatus::Running))
            .map(|snapshot| snapshot.id)
            .collect::<Vec<_>>();
        ids.into_iter()
            .filter_map(|id| self.kill(&id).ok())
            .collect()
    }
}

pub(crate) fn command_env_for_wrapped(
    wrapped: &WrappedCommand,
    base_env: &HashMap<String, String>,
    overrides: &HashMap<String, String>,
) -> Result<HashMap<String, String>, ToolError> {
    let mut env_map = HashMap::with_capacity(base_env.len() + overrides.len() + 4);
    env_map.extend(
        base_env
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    if wrapped.sandboxed {
        let sandbox_home = wrapped.sandbox_home.as_deref().ok_or_else(|| {
            ToolError::ExecutionFailed("sandboxed pty is missing sandbox home".into())
        })?;
        env_map.insert("HOME".to_string(), sandbox_home.display().to_string());
        env_map.insert(
            "XDG_CACHE_HOME".to_string(),
            sandbox_home.join(".cache").display().to_string(),
        );
        env_map.insert(
            "XDG_CONFIG_HOME".to_string(),
            sandbox_home.join(".config").display().to_string(),
        );
        env_map.insert(
            "XDG_DATA_HOME".to_string(),
            sandbox_home.join(".local/share").display().to_string(),
        );
    }
    env_map.extend(
        overrides
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    if wrapped.sandboxed {
        ensure_usable_path(&mut env_map);
        if !wrapped.network_access {
            env_map.insert("RARA_SANDBOX_NETWORK_DISABLED".to_string(), "1".to_string());
        }
    }
    Ok(env_map)
}

fn ensure_usable_path(env_map: &mut HashMap<String, String>) {
    let needs_path = env_map.get("PATH").is_none_or(|value| value.is_empty());
    if needs_path {
        let fallback_path = env::var("PATH")
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "/usr/bin:/bin".to_string());
        env_map.insert("PATH".to_string(), fallback_path);
    }
}

fn ensure_sandbox_home_dirs(sandbox_home: &Path) -> Result<(), ToolError> {
    for dir in [
        sandbox_home,
        &sandbox_home.join(".cache"),
        &sandbox_home.join(".config"),
        &sandbox_home.join(".local"),
        &sandbox_home.join(".local/share"),
    ] {
        std::fs::create_dir_all(dir)?;
    }
    Ok(())
}
