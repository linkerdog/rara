//! Execution of Claude Code plugin command hooks.
//!
//! Spawns shell commands with JSON input on stdin, collects output
//! and exit status, returning a structured result.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;

use crate::types::HookHandler;

/// Output format for hook commands.
#[derive(Debug, Clone, Serialize)]
pub struct HookInput {
    pub session_id: String,
    pub transcript_path: Option<String>,
    pub hook_event: String,
    pub plugin_root: String,
    pub tool_name: Option<String>,
    pub tool_input: Option<Value>,
    pub tool_response: Option<Value>,
    pub last_assistant_message: Option<String>,
    pub is_interrupt: Option<bool>,
    pub prompt: Option<String>,
}

/// Result of executing a command hook.
#[derive(Debug)]
pub struct HookExecutionResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub ok: bool,
}

/// Execute a command hook and return the result.
///
/// The command is spawned with the plugin root in PATH-like context,
/// stdin receives JSON HookInput, and the process is awaited with
/// a configurable timeout.
pub async fn execute_command_hook(
    handler: &HookHandler,
    plugin_root: &PathBuf,
    input: HookInput,
) -> HookExecutionResult {
    let timeout_secs = if handler.timeout > 0 {
        handler.timeout
    } else {
        60 // default 60s
    };

    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(&handler.command)
        .current_dir(plugin_root)
        .env("CLAUDE_PLUGIN_ROOT", plugin_root.to_string_lossy().as_ref())
        .env("PLUGIN_ROOT", plugin_root.to_string_lossy().as_ref())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return HookExecutionResult {
                exit_code: Some(-1),
                stdout: String::new(),
                stderr: format!("failed to spawn hook: {e}"),
                timed_out: false,
                ok: false,
            };
        }
    };

    // Write JSON input to stdin
    if let Some(mut stdin) = child.stdin.take() {
        let json = match serde_json::to_string(&input) {
            Ok(json) => json,
            Err(e) => {
                let _ = child.start_kill();
                return HookExecutionResult {
                    exit_code: Some(-1),
                    stdout: String::new(),
                    stderr: format!("failed to encode hook input: {e}"),
                    timed_out: false,
                    ok: false,
                };
            }
        };
        if let Err(e) = stdin.write_all(json.as_bytes()).await {
            let _ = child.start_kill();
            return HookExecutionResult {
                exit_code: Some(-1),
                stdout: String::new(),
                stderr: format!("failed to write hook input: {e}"),
                timed_out: false,
                ok: false,
            };
        }
        // Close stdin
        drop(stdin);
    }

    // Wait with timeout
    let output = match timeout(Duration::from_secs(timeout_secs), child.wait_with_output()).await {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            return HookExecutionResult {
                exit_code: Some(-1),
                stdout: String::new(),
                stderr: format!("hook process error: {e}"),
                timed_out: false,
                ok: false,
            };
        }
        Err(_) => {
            return HookExecutionResult {
                exit_code: None,
                stdout: String::new(),
                stderr: format!("hook timed out after {timeout_secs}s"),
                timed_out: true,
                ok: false,
            };
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let exit_code = output.status.code();
    let ok = exit_code == Some(0);

    // If stdout contains JSON with "continue": false, treat as blocking
    // even if exit code is 0 (Claude Code compatibility)
    let ok = if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&stdout) {
        parsed
            .get("continue")
            .and_then(|v| v.as_bool())
            .unwrap_or(ok)
    } else {
        ok
    };

    HookExecutionResult {
        exit_code,
        stdout,
        stderr,
        timed_out: false,
        ok,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn executes_simple_echo_hook() {
        let handler = HookHandler {
            r#type: "command".to_string(),
            command: "echo '{\"continue\": true}'".to_string(),
            timeout: 5,
            matcher: None,
            once: false,
        };
        let result = execute_command_hook(
            &handler,
            &PathBuf::from("/tmp"),
            HookInput {
                session_id: "test".to_string(),
                transcript_path: None,
                hook_event: "Stop".to_string(),
                plugin_root: "/tmp".to_string(),
                tool_name: None,
                tool_input: None,
                tool_response: None,
                last_assistant_message: None,
                is_interrupt: None,
                prompt: None,
            },
        )
        .await;
        assert!(result.ok);
    }

    #[tokio::test]
    async fn exposes_plugin_root_environment_aliases() {
        let plugin_root = tempfile::tempdir().expect("plugin root");
        let plugin_root = plugin_root.path().to_path_buf();
        let expected = plugin_root.to_string_lossy();
        let handler = HookHandler {
            r#type: "command".to_string(),
            command: format!(
                "test \"$CLAUDE_PLUGIN_ROOT\" = '{expected}' && test \"$PLUGIN_ROOT\" = '{expected}'"
            ),
            timeout: 5,
            matcher: None,
            once: false,
        };

        let result = execute_command_hook(
            &handler,
            &plugin_root,
            HookInput {
                session_id: "test".to_string(),
                transcript_path: None,
                hook_event: "Stop".to_string(),
                plugin_root: plugin_root.to_string_lossy().to_string(),
                tool_name: None,
                tool_input: None,
                tool_response: None,
                last_assistant_message: None,
                is_interrupt: None,
                prompt: None,
            },
        )
        .await;

        assert!(result.ok);
    }

    #[tokio::test]
    async fn blocks_on_continue_false() {
        let handler = HookHandler {
            r#type: "command".to_string(),
            command: "echo '{\"continue\": false}'".to_string(),
            timeout: 5,
            matcher: None,
            once: false,
        };
        let result = execute_command_hook(
            &handler,
            &PathBuf::from("/tmp"),
            HookInput {
                session_id: "test".to_string(),
                transcript_path: None,
                hook_event: "Stop".to_string(),
                plugin_root: "/tmp".to_string(),
                tool_name: None,
                tool_input: None,
                tool_response: None,
                last_assistant_message: None,
                is_interrupt: None,
                prompt: None,
            },
        )
        .await;
        assert!(!result.ok);
    }

    #[tokio::test]
    async fn non_zero_exit_is_failure() {
        let handler = HookHandler {
            r#type: "command".to_string(),
            command: "exit 1".to_string(),
            timeout: 5,
            matcher: None,
            once: false,
        };
        let result = execute_command_hook(
            &handler,
            &PathBuf::from("/tmp"),
            HookInput {
                session_id: "test".to_string(),
                transcript_path: None,
                hook_event: "Stop".to_string(),
                plugin_root: "/tmp".to_string(),
                tool_name: None,
                tool_input: None,
                tool_response: None,
                last_assistant_message: None,
                is_interrupt: None,
                prompt: None,
            },
        )
        .await;
        assert!(!result.ok);
    }
}
