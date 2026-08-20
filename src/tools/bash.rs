use std::collections::HashMap;
use std::env;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::time::Instant;

use async_trait::async_trait;
use rara_background_tasks::{
    BackgroundTaskListTool, BackgroundTaskRecord, BackgroundTaskStart, BackgroundTaskState,
    BackgroundTaskStatus, BackgroundTaskStatusTool, BackgroundTaskStopTool, BackgroundTaskStore,
    BashStreamKind, append_background_output, read_output_tail,
};
use rara_tool_macros::tool_spec;
use rara_tools::tool::{Tool, ToolCallContext, ToolError, ToolOutputStream, ToolProgressEvent};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::fs;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::{Instant as TokioInstant, MissedTickBehavior};

use crate::sandbox::{SandboxManager, WrappedCommand, sandbox_failure_hint};
use crate::tool_result::model_preview_bash_output;
use crate::tools::bash_readonly::*;

mod outcome;

use outcome::{ProcessTermination, classify_sandbox_failure};

pub struct BashTool {
    pub sandbox: Arc<SandboxManager>,
    pub background_tasks: Arc<BackgroundTaskStore>,
    pub base_env: Arc<HashMap<String, String>>,
    pub sandbox_network_access: Arc<AtomicBool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BashCommandInput {
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub program: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub allow_net: bool,
    #[serde(default)]
    pub run_in_background: bool,
    #[serde(default)]
    pub sandbox_permissions: BashSandboxPermissions,
    #[serde(default)]
    pub justification: Option<String>,
    #[serde(default)]
    pub prefix_rule: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BashSandboxPermissions {
    #[default]
    UseDefault,
    RequireEscalated,
}

impl BashCommandInput {
    pub fn from_value(input: Value) -> Result<Self, ToolError> {
        let parsed: Self = serde_json::from_value(input)
            .map_err(|err| ToolError::InvalidInput(format!("bash payload: {err}")))?;
        let parsed = parsed.normalize_simple_cd_prefix();
        parsed.validate()?;
        Ok(parsed)
    }

    pub fn validate(&self) -> Result<(), ToolError> {
        let has_command = self
            .command
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty());
        let has_program = self
            .program
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty());
        if !has_command && !has_program {
            return Err(ToolError::InvalidInput(
                "bash payload requires either command or program".into(),
            ));
        }
        Ok(())
    }

    pub fn working_dir(&self) -> Result<String, ToolError> {
        match self.cwd.as_ref() {
            Some(cwd) if !cwd.trim().is_empty() => Ok(cwd.clone()),
            _ => Ok(env::current_dir()?.to_string_lossy().to_string()),
        }
    }

    fn normalize_simple_cd_prefix(mut self) -> Self {
        if self.cwd.as_ref().is_some_and(|cwd| !cwd.trim().is_empty())
            || self
                .program
                .as_ref()
                .is_some_and(|value| !value.trim().is_empty())
        {
            return self;
        }

        let Some(command) = self.command.as_deref() else {
            return self;
        };
        let Some((cwd, command)) = parse_simple_cd_prefix(command) else {
            return self;
        };

        self.cwd = Some(cwd);
        self.command = Some(command);
        self
    }

    pub fn summary(&self) -> String {
        if let Some(command) = self
            .command
            .as_ref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        {
            return command.to_string();
        }

        let program = self
            .program
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("<program>");
        if self.args.is_empty() {
            program.to_string()
        } else {
            format!("{program} {}", self.args.join(" "))
        }
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("bash command input should serialize")
    }

    pub fn is_read_only(&self) -> bool {
        if self.allow_net
            || self.run_in_background
            || !self.env.is_empty()
            || self.sandbox_permissions != BashSandboxPermissions::UseDefault
            || self.justification.is_some()
            || !self.prefix_rule.is_empty()
        {
            return false;
        }
        if let Some(command) = self
            .command
            .as_ref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        {
            return shell_command_is_read_only(command);
        }
        self.program
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .is_some_and(|program| argv_is_read_only(program, &self.args))
    }

    pub fn requires_escalated_permissions(&self) -> bool {
        self.sandbox_permissions == BashSandboxPermissions::RequireEscalated
    }

    pub fn approval_prefix(&self) -> Option<String> {
        if !self.prefix_rule.is_empty() {
            return Some(self.prefix_rule.join(" "));
        }
        if let Some(command) = self
            .command
            .as_ref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        {
            let segments = shell_command_prefix_segments(command)?;
            if segments.len() != 1 {
                return None;
            }
            return prefix_from_tokens(&segments[0]);
        }

        let program = self
            .program
            .as_deref()
            .filter(|value| !value.trim().is_empty())?;
        let mut tokens = Vec::with_capacity(self.args.len() + 1);
        tokens.push(program.to_string());
        tokens.extend(self.args.iter().cloned());
        prefix_from_tokens(&tokens)
    }

    pub fn matches_approval_prefix(&self, prefix: &str) -> bool {
        if let Some(command) = self
            .command
            .as_ref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        {
            return shell_command_matches_single_approval_prefix(command, prefix);
        }

        let normalized = self.normalized_approval_summary();
        normalized_summary_matches_prefix(&normalized, prefix)
    }

    pub fn is_allowed_by_approval_prefixes(&self, prefixes: &[String]) -> bool {
        if prefixes.is_empty() {
            return false;
        }
        if prefixes
            .iter()
            .any(|prefix| summary_matches_exact_approval(&self.summary(), prefix))
        {
            return true;
        }

        if let Some(command) = self
            .command
            .as_ref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        {
            return shell_command_allowed_by_approval_prefixes(
                command,
                prefixes,
                self.can_use_read_only_segment_bypass(),
            );
        }

        prefixes
            .iter()
            .any(|prefix| self.matches_approval_prefix(prefix))
    }

    fn can_use_read_only_segment_bypass(&self) -> bool {
        !self.allow_net
            && !self.run_in_background
            && self.env.is_empty()
            && self.sandbox_permissions == BashSandboxPermissions::UseDefault
            && self.justification.is_none()
            && self.prefix_rule.is_empty()
    }

    fn normalized_approval_summary(&self) -> String {
        if let Some(command) = self
            .command
            .as_ref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        {
            if let Some(tokens) = split_shell_segments(command).and_then(|segments| {
                if segments.len() == 1 {
                    tokenize_shell_segment(&segments[0])
                } else {
                    None
                }
            }) {
                return normalized_tokens_summary(&tokens);
            }
            return command.to_string();
        }

        let Some(program) = self
            .program
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        else {
            return self.summary();
        };
        let mut tokens = Vec::with_capacity(self.args.len() + 1);
        tokens.push(program.to_string());
        tokens.extend(self.args.iter().cloned());
        normalized_tokens_summary(&tokens)
    }
}

fn parse_simple_cd_prefix(command: &str) -> Option<(String, String)> {
    let trimmed = command.trim_start();
    let after_cd = trimmed.strip_prefix("cd")?;
    if !after_cd.chars().next().is_some_and(char::is_whitespace) {
        return None;
    }

    let after_cd = after_cd.trim_start();
    let (cwd, rest) = parse_cd_target(after_cd)?;
    if !Path::new(&cwd).is_absolute() {
        return None;
    }

    let rest = rest.trim_start();
    let rest = rest.strip_prefix("&&")?.trim_start();
    if rest.is_empty() {
        return None;
    }

    Some((cwd, rest.to_string()))
}

fn parse_cd_target(input: &str) -> Option<(String, &str)> {
    let quote = input
        .chars()
        .next()
        .filter(|value| *value == '\'' || *value == '"');
    if let Some(quote) = quote {
        let mut chars = input.char_indices();
        chars.next();
        for (idx, ch) in chars {
            if ch == '\\' {
                return None;
            }
            if ch == quote {
                let path = &input[1..idx];
                if path.is_empty() {
                    return None;
                }
                return Some((path.to_string(), &input[idx + quote.len_utf8()..]));
            }
        }
        return None;
    }

    let path_end = input
        .char_indices()
        .find_map(|(idx, ch)| ch.is_whitespace().then_some(idx))
        .unwrap_or(input.len());
    let path = &input[..path_end];
    if path.is_empty()
        || path.chars().any(|ch| {
            matches!(
                ch,
                ';' | '&' | '|' | '<' | '>' | '$' | '`' | '(' | ')' | '{' | '}'
            )
        })
    {
        return None;
    }
    Some((path.to_string(), &input[path_end..]))
}

fn sandbox_command_env(
    sandbox_home: &Path,
    base_env: &HashMap<String, String>,
    overrides: &HashMap<String, String>,
    network_access: bool,
) -> HashMap<String, String> {
    let sandbox_home = sandbox_home.to_string_lossy();
    let mut env_map = HashMap::from([
        ("HOME".to_string(), sandbox_home.to_string()),
        (
            "XDG_CONFIG_HOME".to_string(),
            format!("{sandbox_home}/.config"),
        ),
        (
            "XDG_CACHE_HOME".to_string(),
            format!("{sandbox_home}/.cache"),
        ),
        (
            "XDG_DATA_HOME".to_string(),
            format!("{sandbox_home}/.local/share"),
        ),
    ]);
    env_map.extend(
        base_env
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    // Apply sandbox defaults that survive base_env but can be
    for (k, v) in [("TERM", "dumb"), ("NO_COLOR", "1"), ("PAGER", "cat")] {
        env_map.entry(k.to_string()).or_insert(v.to_string());
    }
    env_map.extend(
        overrides
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    ensure_usable_path(&mut env_map);
    if !network_access {
        env_map.insert("RARA_SANDBOX_NETWORK_DISABLED".to_string(), "1".to_string());
    }
    env_map
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

fn command_env_for_wrapped(
    wrapped: &WrappedCommand,
    base_env: &HashMap<String, String>,
    overrides: &HashMap<String, String>,
) -> Result<HashMap<String, String>, ToolError> {
    if wrapped.sandboxed {
        let sandbox_home = wrapped.sandbox_home.as_deref().ok_or_else(|| {
            ToolError::ExecutionFailed("sandboxed command is missing sandbox home".into())
        })?;
        Ok(sandbox_command_env(
            sandbox_home,
            base_env,
            overrides,
            wrapped.network_access,
        ))
    } else {
        let mut env_map = HashMap::with_capacity(base_env.len() + overrides.len());
        env_map.extend(
            base_env
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        env_map.extend(
            overrides
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
        Ok(env_map)
    }
}

#[tool_spec(
    name = "bash",
    description = "Run a shell command in the sandbox for commands that need process execution. Use the cwd field for the working directory; do not prefix commands with cd. Prefer dedicated RARA tools for file search, file reads, and file edits. Edit files with apply_patch, replace, replace_lines, or write_file; do not use shell redirection, sed -i, awk, perl, heredocs, or ad-hoc scripts to edit files when direct edit tools can do the job. Avoid newline-separated command chaining. If commands are independent and can run in parallel, make multiple bash tool calls in one assistant turn instead of joining them with &&, ;, or pipelines. Do not add 2>&1, head, tail, or grep only to reduce displayed output; RARA preserves stdout/stderr and provides bounded model-facing previews. Commands must be non-interactive: do not start editors, pagers, REPLs, prompts, or TUI programs from bash. For git commits, always supply the message with git commit -m or git commit -F; never run bare git commit and wait for an editor. Keep commands sandboxed unless require_escalated is justified by user request or clear sandbox failure evidence. If a needed test, build, or check is blocked by sandbox limits, inspect the exact denial, try the narrowest viable command, and request require_escalated only when that evidence shows the sandbox is the blocker; do not stop verification just because the first command was denied. Do not re-run the exact same denied sandboxed validation command; either narrow it or switch to require_escalated with a concrete justification when the blocked capability is essential. Use run_in_background for long-running non-interactive commands, then inspect or stop them with background_task_status, background_task_list, and background_task_stop.",
    input_schema = {
        "type": "object",
        "properties": {
            "command": {
                "type": "string",
                "description": "Legacy shell command string. Do not prefix this command with cd; set the cwd field instead. Prefer program+args for new calls. Avoid newline-separated command chaining. Do not join independent validation commands with &&, ;, or pipelines just to run them together; make multiple bash tool calls instead. Do not add 2>&1, head, tail, or grep only to trim output for the model. Do not run interactive editors, pagers, REPLs, prompts, or TUI programs from bash. For git commits, use git commit -m or git commit -F, never bare git commit. Do not use this field for file edits when apply_patch, replace, replace_lines, or write_file can do the job; avoid sed -i, awk, perl, shell redirection, and heredocs for edits. If a validation command is denied by the sandbox, inspect the exact failure and either narrow the command or request escalated permissions instead of giving up. Do not repeat the exact same denied sandboxed validation call."
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
                "description": "Optional working directory override. Defaults to the current turn cwd. Use this instead of prefixing the command with cd."
            },
            "env": {
                "type": "object",
                "additionalProperties": { "type": "string" },
                "description": "Optional environment overrides."
            },
            "allow_net": {
                "type": "boolean",
                "default": false,
                "description": "Request network access for this command. Commands already have network access when sandbox_workspace_write.network_access is enabled in config."
            },
            "run_in_background": {
                "type": "boolean",
                "default": false,
                "description": "Run a long-running non-interactive command as a background task and return a task id immediately. Use background_task_status to inspect output later, background_task_list to find tasks, and background_task_stop to stop them."
            },
            "sandbox_permissions": {
                "type": "string",
                "enum": ["use_default", "require_escalated"],
                "default": "use_default",
                "description": "Sandbox permissions for the command. Defaults to use_default. Set to require_escalated only when the user asked for it or sandbox failure evidence shows the command cannot work inside the sandbox. For tests, builds, and checks, first confirm that the denial is actually caused by sandbox restrictions. If the same essential validation command was already denied for sandbox reasons, prefer require_escalated over repeating the denied sandboxed call."
            },
            "justification": {
                "type": "string",
                "description": "Only set if sandbox_permissions is require_escalated. Ask the user a short question explaining why this command needs to run outside the sandbox and which blocked validation or capability requires it."
            },
            "prefix_rule": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Only set if sandbox_permissions is require_escalated. Suggested scoped command prefix to approve for similar future commands, for example [\"git\", \"push\"] or [\"cargo\", \"test\"]. Do not suggest broad prefixes."
            }
        },
        "anyOf": [
            { "required": ["command"] },
            { "required": ["program"] }
        ]
    }
)]
#[async_trait]
impl Tool for BashTool {
    async fn call(&self, i: Value) -> Result<Value, ToolError> {
        self.call_with_events(i, &mut |_| {}).await
    }

    async fn call_with_events(
        &self,
        i: Value,
        report: &mut (dyn FnMut(ToolProgressEvent) + Send),
    ) -> Result<Value, ToolError> {
        self.call_with_context_events(i, ToolCallContext::default(), report)
            .await
    }

    async fn call_with_context_events(
        &self,
        i: Value,
        context: ToolCallContext,
        report: &mut (dyn FnMut(ToolProgressEvent) + Send),
    ) -> Result<Value, ToolError> {
        let request = BashCommandInput::from_value(i)?;
        let cwd = request
            .cwd
            .as_deref()
            .filter(|cwd| !cwd.trim().is_empty())
            .map_or_else(
                || {
                    context
                        .workspace_root()
                        .map(|cwd| cwd.display().to_string())
                        .map(Ok)
                        .unwrap_or_else(|| request.working_dir())
                },
                |cwd| Ok(cwd.to_string()),
            )?;
        let allow_net = self.sandbox_network_access.load(Ordering::Relaxed) || request.allow_net;
        let wrapped = if let Some(command) = request.command.as_deref() {
            if request.sandbox_permissions == BashSandboxPermissions::RequireEscalated {
                self.sandbox.wrap_unsandboxed_shell_command(command)
            } else {
                self.sandbox
                    .wrap_shell_command(command, &cwd, allow_net)
                    .map_err(|e| {
                        ToolError::ExecutionFailed(format!("{} {}", e, sandbox_failure_hint()))
                    })?
            }
        } else {
            let program = request
                .program
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| ToolError::InvalidInput("program".into()))?;
            if request.sandbox_permissions == BashSandboxPermissions::RequireEscalated {
                self.sandbox
                    .wrap_unsandboxed_exec_command(program, &request.args)
            } else {
                self.sandbox
                    .wrap_exec_command(program, &request.args, &cwd, allow_net)
                    .map_err(|e| {
                        ToolError::ExecutionFailed(format!("{} {}", e, sandbox_failure_hint()))
                    })?
            }
        };
        let command_env = command_env_for_wrapped(&wrapped, &self.base_env, &request.env)?;

        if wrapped.sandboxed && wrapped.sandbox_backend == "macos-seatbelt" {
            let sandbox_home = wrapped.sandbox_home.as_deref().ok_or_else(|| {
                ToolError::ExecutionFailed("sandboxed command is missing sandbox home".into())
            })?;
            ensure_sandbox_home_dirs(sandbox_home).await?;
        }

        let mut command = Command::new(&wrapped.program);
        if wrapped.sandboxed {
            command.env_clear();
        }
        command
            .args(&wrapped.args)
            .current_dir(&cwd)
            .envs(&command_env)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if wrapped.sandboxed {
            command.stdin(Stdio::null());
        }
        configure_process_group(&mut command);
        let started_at = Instant::now();
        let mut child = command.spawn().map_err(|err| {
            if wrapped.sandboxed {
                ToolError::ExecutionFailed(format!(
                    "failed to launch sandbox '{}': {err}. {}",
                    wrapped.program,
                    sandbox_failure_hint()
                ))
            } else {
                ToolError::ExecutionFailed(format!(
                    "failed to launch command '{}': {err}",
                    wrapped.program
                ))
            }
        })?;
        let process_group_id = child.id();

        let sandbox_perm = request.sandbox_permissions;
        if request.run_in_background {
            let (record, stop_rx) = self.background_tasks.start_record(BackgroundTaskStart {
                command: request.summary(),
                program: request
                    .program
                    .as_deref()
                    .filter(|v| !v.trim().is_empty())
                    .map(String::from),
                args: request.args.clone(),
                cwd: Some(cwd.clone()),
                sandboxed: wrapped.sandboxed,
                sandbox_backend: wrapped.sandbox_backend.clone(),
                network_access: wrapped.network_access,
            })?;
            spawn_background_bash_task(
                child,
                wrapped,
                sandbox_perm,
                record.clone(),
                self.background_tasks.clone(),
                stop_rx,
                process_group_id,
            );
            return Ok(json!({
                "stdout": "",
                "stderr": "",
                "exit_code": null,
                "live_streamed": false,
                "sandboxed": record.sandboxed,
                "sandbox_backend": record.sandbox_backend,
                "background_task_id": record.id,
                "output_path": record.output_path,
                "status": record.status,
                "network_access": record.network_access,
            }));
        }

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ToolError::ExecutionFailed("stdout pipe unavailable".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ToolError::ExecutionFailed("stderr pipe unavailable".into()))?;

        let (tx, mut rx) = mpsc::channel(64);
        let stdout_task = tokio::spawn(rara_background_tasks::read_stream_chunks(
            stdout,
            BashStreamKind::Stdout,
            tx.clone(),
        ));
        let stderr_task = tokio::spawn(rara_background_tasks::read_stream_chunks(
            stderr,
            BashStreamKind::Stderr,
            tx,
        ));

        let mut stdout_text = String::new();
        let mut stderr_text = String::new();
        let mut aggregated_output = String::new();
        let mut aggregated_output_stream = None;
        let mut live_streamed = false;
        let mut cancelled = false;
        let mut cancellation_watchdog = Box::pin(tokio::time::sleep(Duration::from_secs(u64::MAX)));
        if !wrapped.sandboxed
            && request.sandbox_permissions != BashSandboxPermissions::RequireEscalated
        {
            let chunk = unsandboxed_execution_warning(&wrapped);
            stderr_text.push_str(&chunk);
            append_aggregated_bash_output(
                &mut aggregated_output,
                &mut aggregated_output_stream,
                BashStreamKind::Stderr,
                &chunk,
            );
            live_streamed = true;
            report(ToolProgressEvent::Output {
                stream: ToolOutputStream::Stderr,
                chunk,
            });
        }
        let mut cancellation_interval = tokio::time::interval(Duration::from_millis(100));
        cancellation_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                chunk = rx.recv() => {
                    let Some((stream, chunk)) = chunk else {
                        break;
                    };
                    if chunk.is_empty() {
                        continue;
                    }
                    live_streamed = true;
                    match stream {
                        BashStreamKind::Stdout => stdout_text.push_str(&chunk),
                        BashStreamKind::Stderr => stderr_text.push_str(&chunk),
                    }
                    append_aggregated_bash_output(
                        &mut aggregated_output,
                        &mut aggregated_output_stream,
                        stream,
                        &chunk,
                    );
                    report(ToolProgressEvent::Output {
                        stream: stream.output_stream(),
                        chunk,
                    });
                }
                _ = cancellation_interval.tick(), if !cancelled => {
                    if context.is_cancelled() {
                        cancelled = true;
                        let chunk = "bash command cancelled by user\n".to_string();
                        stderr_text.push_str(&chunk);
                        append_aggregated_bash_output(
                            &mut aggregated_output,
                            &mut aggregated_output_stream,
                            BashStreamKind::Stderr,
                            &chunk,
                        );
                        live_streamed = true;
                        report(ToolProgressEvent::Output {
                            stream: ToolOutputStream::Stderr,
                            chunk,
                        });
                        rara_background_tasks::kill_child_process_group(process_group_id);
                        let _ = child.start_kill();
                        cancellation_watchdog
                            .as_mut()
                            .reset(TokioInstant::now() + Duration::from_secs(2));
                    }
                }
                _ = &mut cancellation_watchdog, if cancelled => {
                    break;
                }
            }
        }

        let status = if cancelled {
            let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
            None
        } else {
            Some(child.wait().await?)
        };

        if let Some(status) = status {
            stdout_task
                .await
                .map_err(|err| ToolError::ExecutionFailed(err.to_string()))??;
            stderr_task
                .await
                .map_err(|err| ToolError::ExecutionFailed(err.to_string()))??;
            if let Some(path) = wrapped.cleanup_path.as_ref() {
                let _ = fs::remove_file(path).await;
            }
            let termination = ProcessTermination::from_status(&status);
            let exit_code = termination.exit_code();
            let sandbox_failure = classify_sandbox_failure(
                &termination,
                wrapped.sandboxed,
                &wrapped.sandbox_backend,
                &aggregated_output,
            );
            if wrapped.sandboxed
                && let Some(hint) = sandbox_output_hint(&stderr_text)
            {
                stderr_text.push_str(hint);
                append_aggregated_bash_output(
                    &mut aggregated_output,
                    &mut aggregated_output_stream,
                    BashStreamKind::Stderr,
                    hint,
                );
            }
            let duration_ms = started_at.elapsed().as_millis() as u64;
            let model_preview_output =
                model_preview_bash_output(&aggregated_output, exit_code.map(i64::from));

            let mut result = json!({
                "stdout": stdout_text,
                "stderr": stderr_text,
                "aggregated_output": aggregated_output,
                "model_preview_output": model_preview_output,
                "exit_code": exit_code,
                "termination": termination,
                "duration_ms": duration_ms,
                "live_streamed": live_streamed,
                "sandboxed": wrapped.sandboxed,
                "sandbox_backend": wrapped.sandbox_backend,
            });
            if let Some(sandbox_failure) = sandbox_failure {
                result["sandbox_failure"] = json!(sandbox_failure);
            }
            return Ok(result);
        }

        stdout_task.abort();
        stderr_task.abort();
        if let Some(path) = wrapped.cleanup_path.as_ref() {
            let _ = fs::remove_file(path).await;
        }
        Err(ToolError::ExecutionFailed("cancelled by user".into()))
    }
}

fn spawn_background_bash_task(
    mut child: Child,
    wrapped: WrappedCommand,
    sandbox_permissions: BashSandboxPermissions,
    record: BackgroundTaskRecord,
    store: Arc<BackgroundTaskStore>,
    stop_rx: oneshot::Receiver<()>,
    process_group_id: Option<u32>,
) {
    let sandbox_warning =
        if !wrapped.sandboxed && sandbox_permissions != BashSandboxPermissions::RequireEscalated {
            Some(unsandboxed_execution_warning(&wrapped))
        } else {
            None
        };
    let cleanup_path = wrapped.cleanup_path.clone();
    tokio::spawn(async move {
        let result = rara_background_tasks::run_background_bash_task(
            &mut child,
            &record,
            stop_rx,
            process_group_id,
            sandbox_warning.as_deref(),
            cleanup_path.as_deref(),
        )
        .await;
        let (status, exit_code) = match result {
            Ok(code) => {
                if code == Some(0) {
                    (BackgroundTaskStatus::Completed, code)
                } else {
                    (BackgroundTaskStatus::Failed, code)
                }
            }
            Err(err) => {
                let _ = append_background_output(
                    &record.output_path,
                    BashStreamKind::Stderr,
                    &format!("background task failed: {err}\n"),
                )
                .await;
                (BackgroundTaskStatus::Failed, None)
            }
        };
        store.finish(&record.id, status, exit_code);
    });
}

fn append_aggregated_bash_output(
    aggregated_output: &mut String,
    last_stream: &mut Option<BashStreamKind>,
    stream: BashStreamKind,
    chunk: &str,
) {
    if chunk.is_empty() {
        return;
    }
    match stream {
        BashStreamKind::Stdout => aggregated_output.push_str(chunk),
        BashStreamKind::Stderr => {
            if !aggregated_output.is_empty()
                && !aggregated_output.ends_with('\n')
                && !matches!(last_stream, Some(BashStreamKind::Stderr))
            {
                aggregated_output.push('\n');
            }
            for line in chunk.split_inclusive('\n') {
                if aggregated_output.is_empty() || aggregated_output.ends_with('\n') {
                    aggregated_output.push_str("[stderr] ");
                }
                aggregated_output.push_str(line);
            }
        }
    }
    *last_stream = Some(stream);
}

fn sandbox_output_hint(stderr: &str) -> Option<&'static str> {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("sandbox: violation")
        || lower.contains("operation not permitted")
        || lower.contains("command not found")
        || lower.contains("no such file or directory")
        || lower.contains("permission denied")
    {
        Some(
            "\n\nhint: Sandboxed bash appears blocked or missing a runtime path. Prefer direct file tools such as read_file, apply_patch, and replace_lines; ask the user only if a real shell command is required.\n",
        )
    } else {
        None
    }
}

fn unsandboxed_execution_warning(wrapped: &WrappedCommand) -> String {
    format!(
        "warning: command is running without sandbox isolation (backend: {}).\n",
        wrapped.sandbox_backend
    )
}

fn configure_process_group(command: &mut Command) {
    #[cfg(unix)]
    {
        command.process_group(0);
    }
    #[cfg(not(unix))]
    {
        let _ = command;
    }
}

async fn ensure_sandbox_home_dirs(sandbox_home: &Path) -> Result<(), ToolError> {
    for dir in [
        sandbox_home.to_path_buf(),
        sandbox_home.join(".config"),
        sandbox_home.join(".cache"),
        sandbox_home.join(".local"),
        sandbox_home.join(".local/state"),
        sandbox_home.join(".local/share"),
    ] {
        fs::create_dir_all(dir).await?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "bash_tests.rs"]
mod tests;
