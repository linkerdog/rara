use std::sync::atomic::Ordering;

use async_trait::async_trait;
use rara_tool_macros::tool_spec;
use rara_tools::tool::{Tool, ToolCallContext, ToolError, ToolProgressEvent};
use serde_json::{Value, json};

use super::store::START_QUICK_COMPLETION_TIMEOUT as PTY_START_QUICK_COMPLETION_TIMEOUT;
use super::types::{
    PtyCommandInput, PtyKillTool, PtyListTool, PtyReadTool, PtySessionSnapshot, PtyStartTool,
    PtyStatusTool, PtyStopTool, PtyWriteTool,
};
use crate::sandbox::sandbox_failure_hint;
#[tool_spec(
    name = "pty_start",
    description = "Start an interactive PTY session only for commands that need terminal input, terminal control, or an interactive program. For ordinary non-interactive commands, use bash instead. Use the cwd field for the working directory; do not prefix commands with cd. Prefer dedicated RARA tools for file search, file reads, and file edits. PTY sandboxing is platform-dependent and best-effort; with the macOS seatbelt backend, PTY commands currently run directly because sandbox-exec does not preserve interactive PTY stdin reliably. Treat allow_net as a network-access toggle, not a sandbox guarantee. Inspect or stop sessions with pty_status, pty_list, and pty_stop.",
    input_schema = {
        "type": "object",
        "properties": {
            "command": {
                "type": "string",
                "description": "Shell command to run inside a PTY. Do not prefix this command with cd; set the cwd field instead. Use PTY only for interactive commands; use bash for ordinary non-interactive commands."
            },
            "program": {
                "type": "string",
                "description": "Executable to run directly without a shell. Prefer this with args for ordinary commands."
            },
            "args": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Arguments for program."
            },
            "cwd": {
                "type": "string",
                "description": "Optional working directory. Defaults to the current turn cwd. Use this instead of prefixing the command with cd."
            },
            "env": {
                "type": "object",
                "additionalProperties": { "type": "string" },
                "description": "Optional environment overrides."
            },
            "allow_net": {
                "type": "boolean",
                "default": false,
                "description": "Request network access for this PTY session. PTY sessions already have network access when sandbox_workspace_write.network_access is enabled in config."
            },
            "rows": { "type": "integer", "default": 24, "minimum": 1, "maximum": 65535 },
            "cols": { "type": "integer", "default": 120, "minimum": 1, "maximum": 65535 }
        },
        "anyOf": [
            { "required": ["command"] },
            { "required": ["program"] }
        ]
    }
)]
#[async_trait]
impl Tool for PtyStartTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        self.call_with_context_events(input, ToolCallContext::default(), &mut |_| {})
            .await
    }

    async fn call_with_context_events(
        &self,
        input: Value,
        context: ToolCallContext,
        _report: &mut (dyn FnMut(ToolProgressEvent) + Send),
    ) -> Result<Value, ToolError> {
        let request = PtyCommandInput::from_value(input)?;
        let cwd = request
            .cwd
            .as_deref()
            .filter(|cwd| !cwd.trim().is_empty())
            .map_or_else(
                || {
                    context
                        .workspace_root()
                        .map(|cwd| cwd.display().to_string())
                        .unwrap_or_else(|| request.working_dir())
                },
                str::to_string,
            );
        let allow_net = self.sandbox_network_access.load(Ordering::Relaxed) || request.allow_net;
        let (command, wrapped) =
            if let Some(cmd) = request.command.as_deref().filter(|v| !v.trim().is_empty()) {
                let wrapped = self
                    .sandbox
                    .wrap_pty_shell_command(cmd, &cwd, allow_net)
                    .map_err(|err| {
                        ToolError::ExecutionFailed(format!("{} {}", err, sandbox_failure_hint()))
                    })?;
                (cmd.to_string(), wrapped)
            } else {
                let program = request
                    .program
                    .as_deref()
                    .filter(|v| !v.trim().is_empty())
                    .ok_or_else(|| ToolError::InvalidInput("program".into()))?
                    .to_string();
                let summary = request.summary();
                let wrapped = self
                    .sandbox
                    .wrap_pty_exec_command(&program, &request.args, &cwd, allow_net)
                    .map_err(|err| {
                        ToolError::ExecutionFailed(format!("{} {}", err, sandbox_failure_hint()))
                    })?;
                (summary, wrapped)
            };
        let started = self.sessions.start(
            command,
            wrapped,
            cwd,
            &self.base_env,
            request.env,
            request.rows,
            request.cols,
        )?;
        self.sessions
            .wait_for_quick_completion(&started.id, PTY_START_QUICK_COMPLETION_TIMEOUT)
            .await
            .into_json(12_000)
            .await
    }
}

#[tool_spec(
    name = "pty_read",
    description = "Read recent output from a PTY session started with pty_start.",
    input_schema = {
        "type": "object",
        "properties": {
            "session_id": { "type": "string" },
            "tail_bytes": { "type": "integer", "default": 12000, "minimum": 1 }
        },
        "required": ["session_id"]
    }
)]
#[async_trait]
impl Tool for PtyReadTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        let session_id = input["session_id"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidInput("session_id".into()))?;
        let tail_bytes = input
            .get("tail_bytes")
            .and_then(Value::as_u64)
            .unwrap_or(12_000)
            .min(1_000_000) as usize;
        self.sessions
            .get(session_id)
            .ok_or_else(|| {
                ToolError::InvalidInput(format!("unknown pty session id: {session_id}"))
            })?
            .into_json(tail_bytes)
            .await
    }
}

#[tool_spec(
    name = "pty_list",
    description = "List PTY sessions started with pty_start. Use this before starting duplicate interactive work when session state is unclear.",
    input_schema = {
        "type": "object",
        "properties": {}
    }
)]
#[async_trait]
impl Tool for PtyListTool {
    async fn call(&self, _input: Value) -> Result<Value, ToolError> {
        let sessions = self
            .sessions
            .list()
            .into_iter()
            .map(PtySessionSnapshot::metadata_json)
            .collect::<Vec<_>>();
        Ok(json!({ "sessions": sessions }))
    }
}

#[tool_spec(
    name = "pty_status",
    description = "Inspect a PTY session started with pty_start and read the tail of its output.",
    input_schema = {
        "type": "object",
        "properties": {
            "session_id": { "type": "string" },
            "tail_bytes": { "type": "integer", "default": 12000, "minimum": 1 }
        },
        "required": ["session_id"]
    }
)]
#[async_trait]
impl Tool for PtyStatusTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        PtyReadTool {
            sessions: self.sessions.clone(),
        }
        .call(input)
        .await
    }
}

#[tool_spec(
    name = "pty_write",
    description = "Write input to a running PTY session started with pty_start.",
    input_schema = {
        "type": "object",
        "properties": {
            "session_id": { "type": "string" },
            "input": { "type": "string", "description": "Text to write, including newlines or control characters when needed." }
        },
        "required": ["session_id", "input"]
    }
)]
#[async_trait]
impl Tool for PtyWriteTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        let session_id = input["session_id"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidInput("session_id".into()))?;
        let text = input["input"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidInput("input".into()))?;
        self.sessions
            .write(session_id, text)?
            .into_json(12_000)
            .await
    }
}

#[tool_spec(
    name = "pty_kill",
    description = "Kill a PTY session started with pty_start.",
    input_schema = {
        "type": "object",
        "properties": {
            "session_id": { "type": "string" }
        },
        "required": ["session_id"]
    }
)]
#[async_trait]
impl Tool for PtyKillTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        let session_id = input["session_id"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidInput("session_id".into()))?;
        self.sessions.kill(session_id)?.into_json(12_000).await
    }
}

#[tool_spec(
    name = "pty_stop",
    description = "Stop one PTY session, or all running PTY sessions when session_id is omitted.",
    input_schema = {
        "type": "object",
        "properties": {
            "session_id": {
                "type": "string",
                "description": "PTY session id returned by pty_start. Omit to stop all running PTY sessions."
            }
        }
    }
)]
#[async_trait]
impl Tool for PtyStopTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        if let Some(session_id) = input.get("session_id").and_then(Value::as_str) {
            let session = self.sessions.kill(session_id)?;
            return Ok(json!({ "stopped": [session.metadata_json()] }));
        }
        let stopped = self
            .sessions
            .kill_all()
            .into_iter()
            .map(PtySessionSnapshot::metadata_json)
            .collect::<Vec<_>>();
        Ok(json!({ "stopped": stopped }))
    }
}
