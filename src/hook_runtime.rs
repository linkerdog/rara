//! In-process hook runtime that subscribes to the RuntimeEventBus
//! and dispatches matching AgentEvent variants to registered hook callbacks.
//!
//! Hooks are declared through the control plane (HookControlRequest::Declare)
//! and can be registered at any time — before or after calling `start`.
//! The dispatch loop runs on a dedicated Tokio task and matches events
//! against the current set of registered hooks.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::agent::AgentEvent;
use crate::runtime_control::HookLifecycle;
use crate::runtime_event_bus::RuntimeEventBus;

/// A registered in-process hook combining a lifecycle trigger and a callback.
type HookCallback = Box<dyn Fn(&AgentEvent) + Send + Sync>;

struct HookEntry {
    lifecycle: HookLifecycle,
    description: String,
    callback: HookCallback,
}

/// In-process hook dispatch runtime.
///
/// Hooks are stored behind an `Arc<RwLock<HashMap<...>>>` so that the
/// control-plane can register / unregister hooks while the dispatch
/// loop is already running.
pub struct HookRuntime {
    bus: Arc<RuntimeEventBus>,
    hooks: Arc<tokio::sync::RwLock<HashMap<String, HookEntry>>>,
    started: AtomicBool,
}

impl HookRuntime {
    pub fn new(bus: Arc<RuntimeEventBus>) -> Self {
        Self {
            bus,
            hooks: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            started: AtomicBool::new(false),
        }
    }

    /// Register an in-process hook.  Safe to call while `start` is running.
    pub async fn register(
        &self,
        hook_id: String,
        lifecycle: HookLifecycle,
        description: String,
        callback: HookCallback,
    ) {
        self.hooks.write().await.insert(
            hook_id,
            HookEntry {
                lifecycle,
                description,
                callback,
            },
        );
    }

    /// Unregister a previously declared hook by id.
    pub async fn unregister(&self, hook_id: &str) -> bool {
        self.hooks.write().await.remove(hook_id).is_some()
    }

    /// Return the number of registered hooks.
    pub async fn hook_count(&self) -> usize {
        self.hooks.read().await.len()
    }

    /// Start the hook dispatch loop on a dedicated Tokio task.
    ///
    /// Returns immediately if already started (idempotent).
    /// The returned handle can be aborted to stop hook processing.
    /// Callers may safely call `register` / `unregister` after this returns.
    pub fn start(&self) -> Option<tokio::task::JoinHandle<()>> {
        if self.started.swap(true, Ordering::SeqCst) {
            return None;
        }
        let hooks = Arc::clone(&self.hooks);
        let bus = self.bus.clone();

        let handle = tokio::task::spawn(async move {
            let mut rx = bus.subscribe();
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        let guard = hooks.read().await;
                        for entry in guard.values() {
                            if lifecycle_matches(&entry.lifecycle, &event) {
                                (entry.callback)(&event);
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }
        });
        Some(handle)
    }
}

/// Map an AgentEvent variant to the HookLifecycle it corresponds to.
///
/// `AgentStart` deliberately returns `None` because it fires once per agent
/// turn, not once per session.  The session-level lifecycle is managed by the
/// session control plane, not by per-turn agent events.
fn lifecycle_for_event(event: &AgentEvent) -> Option<HookLifecycle> {
    match event {
        AgentEvent::AgentStart => None,
        AgentEvent::AgentStop { .. } => Some(HookLifecycle::Stop),
        AgentEvent::ToolUse { .. } => Some(HookLifecycle::PreToolUse),
        AgentEvent::ToolResult { .. } => Some(HookLifecycle::PostToolUse),
        AgentEvent::ModelRequest { .. } | AgentEvent::ModelResponse { .. } => None,
        AgentEvent::Status(_)
        | AgentEvent::AssistantText(_)
        | AgentEvent::AssistantDelta(_)
        | AgentEvent::AssistantThinkingDelta(_)
        | AgentEvent::ToolProgress { .. }
        | AgentEvent::MemoryAction { .. }
        | AgentEvent::McpStatusUpdated(_)
        | AgentEvent::McpStatusLoadFailed { .. }
        | AgentEvent::TodoUpdated(_)
        | AgentEvent::AgentError { .. } => None,
    }
}

fn lifecycle_matches(lifecycle: &HookLifecycle, event: &AgentEvent) -> bool {
    lifecycle_for_event(event).as_ref() == Some(lifecycle)
}

/// Result of executing a command-type hook.
#[derive(Debug, Clone)]
pub struct HookRunResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub error: Option<String>,
    pub duration_ms: u64,
}

/// Spawn a background thread that executes `script_path` as a hook,
/// writing `input_json` to stdin.  Uses a separate stdin-writer thread
/// to avoid pipe-buffer deadlock.  Kills the child on timeout.
fn spawn_command_hook(
    script_path: std::path::PathBuf,
    input_json: String,
    cwd: std::path::PathBuf,
    timeout_secs: u64,
) {
    std::thread::spawn(move || {
        let result = run_command_hook(&script_path, &input_json, &cwd, timeout_secs);
        match result {
            Some(ref r) if r.error.is_some() => {
                eprintln!(
                    "command hook failed {}: {} ({}ms)",
                    script_path.display(),
                    r.error.as_ref().unwrap(),
                    r.duration_ms
                );
            }
            Some(ref r) => {
                eprintln!(
                    "command hook {} exit={:?} {}ms ({}b stdout)",
                    script_path.display(),
                    r.exit_code,
                    r.duration_ms,
                    r.stdout.len()
                );
            }
            None => {}
        }
    });
}

/// Execute a shell script as a hook, feeding `input_json` to stdin.
///
/// The script runs with `cwd` set to the workspace root and a timeout of
/// `timeout_secs`.  If the script doesn't exist, returns `None`.
/// On timeout the child process is killed and the wait thread is joined.
pub fn run_command_hook(
    script_path: &std::path::Path,
    input_json: &str,
    cwd: &std::path::Path,
    timeout_secs: u64,
) -> Option<HookRunResult> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    if !script_path.exists() {
        return None;
    }
    let started = std::time::Instant::now();

    let mut child = match Command::new(script_path)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return Some(HookRunResult {
                exit_code: None,
                stdout: String::new(),
                stderr: String::new(),
                error: Some(e.to_string()),
                duration_ms: started.elapsed().as_millis() as u64,
            });
        }
    };

    // Write stdin in a separate thread to avoid deadlock when input exceeds
    // the pipe buffer and the child is also writing to stdout/stderr.
    if let Some(mut stdin) = child.stdin.take() {
        let stdin_bytes: Vec<u8> = input_json.as_bytes().to_vec();
        std::thread::spawn(move || {
            let _ = stdin.write_all(&stdin_bytes);
            // stdin is dropped here, closing the pipe
        });
    }

    // Remember the child PID so we can kill it on timeout.
    let child_id = child.id();

    let (tx, rx) = std::sync::mpsc::channel();
    let wait_thread = std::thread::spawn(move || {
        let output = child.wait_with_output();
        let _ = tx.send(output);
    });

    match rx.recv_timeout(std::time::Duration::from_secs(timeout_secs)) {
        Ok(Ok(output)) => {
            let _ = wait_thread.join();
            Some(HookRunResult {
                exit_code: output.status.code(),
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                error: None,
                duration_ms: started.elapsed().as_millis() as u64,
            })
        }
        Ok(Err(e)) => {
            let _ = wait_thread.join();
            Some(HookRunResult {
                exit_code: None,
                stdout: String::new(),
                stderr: String::new(),
                error: Some(e.to_string()),
                duration_ms: started.elapsed().as_millis() as u64,
            })
        }
        Err(_timeout) => {
            // Kill the child process so the wait thread can join
            let _ = std::process::Command::new("kill")
                .arg(child_id.to_string())
                .output();
            let _ = wait_thread.join();
            Some(HookRunResult {
                exit_code: None,
                stdout: String::new(),
                stderr: String::new(),
                error: Some(format!("hook timed out after {timeout_secs}s")),
                duration_ms: started.elapsed().as_millis() as u64,
            })
        }
    }
}

/// Build a `HookCallback` that spawns `script_path` as a fire-and-forget
/// command-type hook in a background thread.  The dispatch loop is never
/// blocked.
pub fn make_command_hook(
    script_path: std::path::PathBuf,
    cwd: std::path::PathBuf,
    timeout_secs: u64,
) -> HookCallback {
    Box::new(move |_event: &AgentEvent| {
        let input_json = "{}".to_string();
        spawn_command_hook(script_path.clone(), input_json, cwd.clone(), timeout_secs);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime_event_bus::RuntimeEventBus;

    #[test]
    fn test_lifecycle_mapping_agent_start_is_none() {
        assert_eq!(lifecycle_for_event(&AgentEvent::AgentStart), None);
    }

    #[test]
    fn test_lifecycle_mapping_agent_stop_tool_use() {
        assert_eq!(
            lifecycle_for_event(&AgentEvent::AgentStop {
                reason: "done".into()
            }),
            Some(HookLifecycle::Stop)
        );
        assert_eq!(
            lifecycle_for_event(&AgentEvent::ToolUse {
                name: "bash".into(),
                input: serde_json::json!({}),
            }),
            Some(HookLifecycle::PreToolUse)
        );
        assert_eq!(
            lifecycle_for_event(&AgentEvent::ToolResult {
                name: "bash".into(),
                content: "ok".into(),
                is_error: false,
            }),
            Some(HookLifecycle::PostToolUse)
        );
    }

    #[test]
    fn test_non_mapped_events() {
        assert_eq!(
            lifecycle_for_event(&AgentEvent::Status("ready".into())),
            None
        );
        assert_eq!(
            lifecycle_for_event(&AgentEvent::AssistantDelta("txt".into())),
            None
        );
        assert_eq!(
            lifecycle_for_event(&AgentEvent::ModelRequest {
                model: "gpt-4".into(),
                input_tokens: 10,
            }),
            None
        );
    }

    #[tokio::test]
    async fn test_register_and_unregister() {
        let bus = Arc::new(RuntimeEventBus::new(4));
        let runtime = HookRuntime::new(bus);

        assert_eq!(runtime.hook_count().await, 0);

        runtime
            .register(
                "hook-1".into(),
                HookLifecycle::SessionStart,
                "test hook".into(),
                Box::new(|_| {}),
            )
            .await;
        assert_eq!(runtime.hook_count().await, 1);

        assert!(runtime.unregister("hook-1").await);
        assert_eq!(runtime.hook_count().await, 0);

        assert!(!runtime.unregister("hook-1").await);
    }
}
