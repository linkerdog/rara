use std::collections::HashMap;
use std::env;
use std::io::SeekFrom;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use async_trait::async_trait;
use rara_tool_macros::tool_spec;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::fs;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::{Instant as TokioInstant, MissedTickBehavior};
use uuid::Uuid;

use crate::sandbox::{SandboxManager, WrappedCommand, sandbox_failure_hint};
use crate::tool::{Tool, ToolCallContext, ToolError, ToolOutputStream, ToolProgressEvent};
use crate::tool_result::model_preview_bash_output;

pub struct BashTool {
    pub sandbox: Arc<SandboxManager>,
    pub background_tasks: Arc<BackgroundTaskStore>,
    pub base_env: Arc<HashMap<String, String>>,
    pub sandbox_network_access: bool,
}

pub struct BackgroundTaskStatusTool {
    pub background_tasks: Arc<BackgroundTaskStore>,
}

pub struct BackgroundTaskListTool {
    pub background_tasks: Arc<BackgroundTaskStore>,
}

pub struct BackgroundTaskStopTool {
    pub background_tasks: Arc<BackgroundTaskStore>,
}

#[derive(Debug, Clone)]
pub struct BackgroundTaskStore {
    dir: PathBuf,
    tasks: Arc<Mutex<HashMap<String, BackgroundTaskRecord>>>,
    stop_signals: Arc<Mutex<HashMap<String, oneshot::Sender<()>>>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackgroundTaskRecord {
    id: String,
    command: String,
    output_path: PathBuf,
    status: BackgroundTaskStatus,
    exit_code: Option<i32>,
    sandboxed: bool,
    sandbox_backend: String,
    network_access: bool,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskStatus {
    Running,
    Completed,
    Failed,
    Killed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BashStreamKind {
    Stdout,
    Stderr,
}

impl BashStreamKind {
    fn output_stream(self) -> ToolOutputStream {
        match self {
            Self::Stdout => ToolOutputStream::Stdout,
            Self::Stderr => ToolOutputStream::Stderr,
        }
    }
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
            let segments = split_shell_segments(command)?;
            if segments.len() != 1 {
                return None;
            }
            let tokens = tokenize_shell_segment(&segments[0])?;
            return prefix_from_tokens(&tokens);
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
        let normalized = self.normalized_approval_summary();
        normalized == prefix
            || normalized
                .strip_prefix(prefix)
                .is_some_and(|suffix| suffix.starts_with(char::is_whitespace))
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

fn prefix_from_tokens(tokens: &[String]) -> Option<String> {
    let program = tokens.first()?;
    let program = command_basename(program);
    if let Some(subcommand) = approval_subcommand_token(program, &tokens[1..]) {
        Some(format!("{program} {subcommand}"))
    } else {
        Some(program.to_string())
    }
}

fn normalized_tokens_summary(tokens: &[String]) -> String {
    let Some(program) = tokens.first() else {
        return String::new();
    };
    let program = command_basename(program);
    let rest = &tokens[1..];
    let args = approval_subcommand_index(program, rest)
        .map(|index| rest[index..].iter().cloned().collect::<Vec<_>>())
        .unwrap_or_else(|| rest.to_vec());
    std::iter::once(program.to_string())
        .chain(args)
        .collect::<Vec<_>>()
        .join(" ")
}

fn command_basename(command: &str) -> &str {
    Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(command)
}

fn approval_subcommand_token<'a>(program: &str, args: &'a [String]) -> Option<&'a str> {
    approval_subcommand_index(program, args).and_then(|index| args.get(index).map(String::as_str))
}

fn approval_subcommand_index(program: &str, args: &[String]) -> Option<usize> {
    match program {
        "git" => skip_known_global_options(
            args,
            &["--no-pager", "--no-optional-locks"],
            &["-C", "-c", "--git-dir", "--work-tree"],
        ),
        "docker" => skip_known_global_options(
            args,
            &["--debug", "--tls", "--tlsverify"],
            &["--config", "--context", "--host", "-H", "--log-level"],
        ),
        _ => args.first().map(|_| 0),
    }
}

fn skip_known_global_options(
    args: &[String],
    valueless_options: &[&str],
    value_options: &[&str],
) -> Option<usize> {
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        if valueless_options.contains(&arg) {
            index += 1;
        } else if value_options.contains(&arg) {
            index += 2;
        } else if value_options
            .iter()
            .any(|option| arg.starts_with(&format!("{option}=")))
        {
            index += 1;
        } else if arg.starts_with('-') {
            index += 1;
        } else {
            return Some(index);
        }
    }
    None
}

fn shell_command_is_read_only(command: &str) -> bool {
    if command.contains('\n')
        || command.contains('`')
        || command.contains("$(")
        || command.contains('>')
    {
        return false;
    }
    split_shell_segments(command)
        .filter(|segments| !segments.is_empty())
        .is_some_and(|segments| {
            segments.into_iter().all(|segment| {
                tokenize_shell_segment(&segment).is_some_and(|tokens| {
                    if tokens.is_empty() {
                        return false;
                    }
                    argv_is_read_only(&tokens[0], &tokens[1..])
                })
            })
        })
}

fn argv_is_read_only(program: &str, args: &[String]) -> bool {
    let program = Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program);
    match program {
        "pwd" | "ls" | "tree" | "cat" | "head" | "tail" | "wc" | "stat" | "file" | "du" | "df"
        | "which" | "type" | "whereis" | "uname" => true,
        "rg" | "grep" => !args
            .iter()
            .any(|arg| matches!(arg.as_str(), "--files-with-matches=")),
        "sed" => !args.iter().any(|arg| {
            arg == "-i"
                || arg.starts_with("-i.")
                || arg == "--in-place"
                || arg.starts_with("--in-place=")
        }),
        "find" => !args.iter().any(|arg| {
            matches!(
                arg.as_str(),
                "-delete" | "-exec" | "-execdir" | "-ok" | "-okdir"
            )
        }),
        "fd" | "fdfind" => !args.iter().any(|arg| {
            matches!(
                arg.as_str(),
                "-x" | "--exec" | "-X" | "--exec-batch" | "--list-details"
            )
        }),
        "git" => git_args_are_read_only(args),
        "docker" => docker_args_are_read_only(args),
        "pyright" => !args
            .iter()
            .any(|arg| matches!(arg.as_str(), "--watch" | "-w")),
        _ => false,
    }
}

fn git_args_are_read_only(args: &[String]) -> bool {
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--no-pager" | "--no-optional-locks" => index += 1,
            "-C" | "-c" | "--git-dir" | "--work-tree" => return false,
            value if value.starts_with('-') => return false,
            _ => break,
        }
    }
    let Some(subcommand) = args.get(index).map(String::as_str) else {
        return false;
    };
    let rest = &args[index + 1..];
    match subcommand {
        "diff" | "log" | "show" | "shortlog" | "status" | "blame" | "ls-files" | "merge-base"
        | "rev-parse" | "rev-list" | "describe" | "cat-file" | "for-each-ref" | "grep" => true,
        "stash" => rest.first().is_some_and(|value| value == "list"),
        "remote" => rest.is_empty() || rest == ["-v"] || rest == ["--verbose"],
        "config" => rest.first().is_some_and(|value| value == "--get"),
        "reflog" => !rest
            .iter()
            .any(|value| matches!(value.as_str(), "expire" | "delete" | "exists")),
        "branch" => {
            rest.is_empty()
                || rest.iter().all(|value| {
                    matches!(
                        value.as_str(),
                        "--list" | "-l" | "-a" | "--all" | "-r" | "--remotes" | "-v" | "-vv"
                    )
                })
        }
        _ => false,
    }
}

fn docker_args_are_read_only(args: &[String]) -> bool {
    args.first()
        .is_some_and(|value| matches!(value.as_str(), "ps" | "images" | "logs" | "inspect"))
}

fn split_shell_segments(command: &str) -> Option<Vec<String>> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(ch) = chars.next() {
        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            }
            current.push(ch);
            continue;
        }
        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                current.push(ch);
            }
            ';' | '|' => {
                push_shell_segment(&mut segments, &mut current);
            }
            '&' if chars.peek() == Some(&'&') => {
                chars.next();
                push_shell_segment(&mut segments, &mut current);
            }
            '&' => return None,
            _ => current.push(ch),
        }
    }
    if quote.is_some() {
        return None;
    }
    push_shell_segment(&mut segments, &mut current);
    Some(segments)
}

fn push_shell_segment(segments: &mut Vec<String>, current: &mut String) {
    let segment = current.trim();
    if !segment.is_empty() {
        segments.push(segment.to_string());
    }
    current.clear();
}

fn tokenize_shell_segment(segment: &str) -> Option<Vec<String>> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = segment.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(ch) = chars.next() {
        match quote {
            Some(active_quote) => {
                if ch == active_quote {
                    quote = None;
                } else if ch == '\\' && active_quote == '"' {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                } else {
                    current.push(ch);
                }
            }
            None => match ch {
                '\'' | '"' => quote = Some(ch),
                '\\' => {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                }
                '<' => return None,
                value if value.is_whitespace() => {
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                }
                _ => current.push(ch),
            },
        }
    }
    if quote.is_some() {
        return None;
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    Some(tokens)
}

impl BackgroundTaskStore {
    pub fn new(dir: PathBuf) -> Result<Self, ToolError> {
        std::fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            tasks: Arc::new(Mutex::new(HashMap::new())),
            stop_signals: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    fn start_record(
        &self,
        command: String,
        sandboxed: bool,
        sandbox_backend: String,
        network_access: bool,
    ) -> Result<(BackgroundTaskRecord, oneshot::Receiver<()>), ToolError> {
        let id = format!("bash-{}", Uuid::new_v4());
        let output_path = self.dir.join(format!("{id}.log"));
        let record = BackgroundTaskRecord {
            id: id.clone(),
            command,
            output_path,
            status: BackgroundTaskStatus::Running,
            exit_code: None,
            sandboxed,
            sandbox_backend,
            network_access,
        };
        let (stop_tx, stop_rx) = oneshot::channel();
        self.tasks
            .lock()
            .expect("background task store lock")
            .insert(id.clone(), record.clone());
        self.stop_signals
            .lock()
            .expect("background task stop signal lock")
            .insert(id, stop_tx);
        Ok((record, stop_rx))
    }

    fn finish(&self, id: &str, status: BackgroundTaskStatus, exit_code: Option<i32>) {
        if let Some(record) = self
            .tasks
            .lock()
            .expect("background task store lock")
            .get_mut(id)
        {
            if !matches!(record.status, BackgroundTaskStatus::Killed) {
                record.status = status;
            }
            record.exit_code = exit_code;
        }
        self.stop_signals
            .lock()
            .expect("background task stop signal lock")
            .remove(id);
    }

    fn get(&self, id: &str) -> Option<BackgroundTaskRecord> {
        self.tasks
            .lock()
            .expect("background task store lock")
            .get(id)
            .cloned()
    }

    fn list(&self) -> Vec<BackgroundTaskRecord> {
        let mut records = self
            .tasks
            .lock()
            .expect("background task store lock")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        records.sort_by(|left, right| left.id.cmp(&right.id));
        records
    }

    fn stop(&self, id: &str) -> Result<BackgroundTaskRecord, ToolError> {
        let mut tasks = self.tasks.lock().expect("background task store lock");
        let record = tasks
            .get_mut(id)
            .ok_or_else(|| ToolError::InvalidInput(format!("unknown task id: {id}")))?;
        if !matches!(record.status, BackgroundTaskStatus::Running) {
            return Ok(record.clone());
        }
        record.status = BackgroundTaskStatus::Killed;
        let stopped = record.clone();
        drop(tasks);

        if let Some(stop) = self
            .stop_signals
            .lock()
            .expect("background task stop signal lock")
            .remove(id)
        {
            let _ = stop.send(());
        }
        Ok(stopped)
    }

    fn stop_all(&self) -> Vec<BackgroundTaskRecord> {
        let ids = self
            .list()
            .into_iter()
            .filter(|record| matches!(record.status, BackgroundTaskStatus::Running))
            .map(|record| record.id)
            .collect::<Vec<_>>();
        ids.into_iter()
            .filter_map(|id| self.stop(&id).ok())
            .collect()
    }
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
            "XDG_STATE_HOME".to_string(),
            format!("{sandbox_home}/.local/state"),
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
    let needs_path = env_map.get("PATH").map_or(true, |value| value.is_empty());
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
    description = "Run a shell command in the sandbox for commands that need process execution. Prefer dedicated RARA tools for file search, file reads, and file edits; do not use shell redirection, sed, awk, perl, or ad-hoc scripts to edit files when apply_patch or direct file tools can do the job. Use the cwd field instead of prepending cd. Avoid newline-separated command chaining. If commands are independent and can run in parallel, make multiple bash tool calls in one assistant turn instead of joining them with &&, ;, or pipelines. Do not add 2>&1, head, tail, or grep only to reduce displayed output; RARA preserves stdout/stderr and provides bounded model-facing previews. Commands must be non-interactive: do not start editors, pagers, REPLs, prompts, or TUI programs from bash. For git commits, always supply the message with git commit -m or git commit -F; never run bare git commit and wait for an editor. Keep commands sandboxed unless require_escalated is justified by user request or clear sandbox failure evidence. Use run_in_background for long-running non-interactive commands, then inspect or stop them with background_task_status, background_task_list, and background_task_stop.",
    input_schema = {
        "type": "object",
        "properties": {
            "command": {
                "type": "string",
                "description": "Legacy shell command string. Prefer program+args for new calls. Avoid newline-separated command chaining. Do not join independent validation commands with &&, ;, or pipelines just to run them together; make multiple bash tool calls instead. Do not add 2>&1, head, tail, or grep only to trim output for the model. Do not run interactive editors, pagers, REPLs, prompts, or TUI programs from bash. For git commits, use git commit -m or git commit -F, never bare git commit. Do not use this field for file edits when apply_patch or direct file tools can do the job."
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
                "description": "Optional working directory override. Defaults to the current turn cwd; prefer this over prepending cd to a command."
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
                "description": "Sandbox permissions for the command. Defaults to use_default. Set to require_escalated only when the user asked for it or sandbox failure evidence shows the command cannot work inside the sandbox."
            },
            "justification": {
                "type": "string",
                "description": "Only set if sandbox_permissions is require_escalated. Ask the user a short question explaining why this command needs to run outside the sandbox."
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
        let cwd = request.working_dir()?;
        let allow_net = self.sandbox_network_access || request.allow_net;
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

        if request.run_in_background {
            let (record, stop_rx) = self.background_tasks.start_record(
                request.summary(),
                wrapped.sandboxed,
                wrapped.sandbox_backend.clone(),
                wrapped.network_access,
            )?;
            spawn_background_bash_task(
                child,
                wrapped,
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
        let stdout_task = tokio::spawn(read_stream_chunks(
            stdout,
            BashStreamKind::Stdout,
            tx.clone(),
        ));
        let stderr_task = tokio::spawn(read_stream_chunks(stderr, BashStreamKind::Stderr, tx));

        let mut stdout_text = String::new();
        let mut stderr_text = String::new();
        let mut aggregated_output = String::new();
        let mut aggregated_output_stream = None;
        let mut live_streamed = false;
        let mut cancelled = false;
        let mut cancellation_watchdog = Box::pin(tokio::time::sleep(Duration::from_secs(u64::MAX)));
        if !wrapped.sandboxed {
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
                        kill_child_process_group(process_group_id);
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
            if wrapped.sandboxed {
                if let Some(hint) = sandbox_output_hint(&stderr_text) {
                    stderr_text.push_str(hint);
                    append_aggregated_bash_output(
                        &mut aggregated_output,
                        &mut aggregated_output_stream,
                        BashStreamKind::Stderr,
                        hint,
                    );
                }
            }
            let duration_ms = started_at.elapsed().as_millis() as u64;
            let model_preview_output =
                model_preview_bash_output(&aggregated_output, status.code().map(i64::from));

            return Ok(json!({
                "stdout": stdout_text,
                "stderr": stderr_text,
                "aggregated_output": aggregated_output,
                "model_preview_output": model_preview_output,
                "exit_code": status.code(),
                "duration_ms": duration_ms,
                "live_streamed": live_streamed,
                "sandboxed": wrapped.sandboxed,
                "sandbox_backend": wrapped.sandbox_backend,
            }));
        }

        stdout_task.abort();
        stderr_task.abort();
        if let Some(path) = wrapped.cleanup_path.as_ref() {
            let _ = fs::remove_file(path).await;
        }
        Err(ToolError::ExecutionFailed("cancelled by user".into()))
    }
}

#[tool_spec(
    name = "background_task_list",
    description = "List background bash tasks started with bash run_in_background. Use this before starting duplicate long-running work when task state is unclear.",
    input_schema = {
        "type": "object",
        "properties": {}
    }
)]
#[async_trait]
impl Tool for BackgroundTaskListTool {
    async fn call(&self, _input: Value) -> Result<Value, ToolError> {
        Ok(json!({
            "tasks": self.background_tasks.list(),
        }))
    }
}

#[tool_spec(
    name = "background_task_status",
    description = "Inspect a background bash task started with bash run_in_background and read the tail of its output.",
    input_schema = {
        "type": "object",
        "properties": {
            "task_id": {
                "type": "string",
                "description": "Background task id returned by bash run_in_background."
            },
            "tail_bytes": {
                "type": "integer",
                "minimum": 1,
                "default": 12000,
                "description": "Maximum number of output bytes to return from the end of the task log."
            }
        },
        "required": ["task_id"]
    }
)]
#[async_trait]
impl Tool for BackgroundTaskStatusTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        let task_id = input["task_id"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidInput("task_id".into()))?;
        let tail_bytes = input
            .get("tail_bytes")
            .and_then(Value::as_u64)
            .unwrap_or(12_000)
            .min(1_000_000) as usize;
        let record = self
            .background_tasks
            .get(task_id)
            .ok_or_else(|| ToolError::InvalidInput(format!("unknown task id: {task_id}")))?;
        let output = read_output_tail(&record.output_path, tail_bytes).await?;

        Ok(json!({
            "task_id": record.id,
            "command": record.command,
            "status": record.status,
            "exit_code": record.exit_code,
            "output_path": record.output_path,
            "output": output,
            "sandboxed": record.sandboxed,
            "sandbox_backend": record.sandbox_backend,
            "network_access": record.network_access,
        }))
    }
}

#[tool_spec(
    name = "background_task_stop",
    description = "Stop one background bash task, or all running background bash tasks when task_id is omitted.",
    input_schema = {
        "type": "object",
        "properties": {
            "task_id": {
                "type": "string",
                "description": "Background task id returned by bash run_in_background. Omit to stop all running background bash tasks."
            }
        }
    }
)]
#[async_trait]
impl Tool for BackgroundTaskStopTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        if let Some(task_id) = input.get("task_id").and_then(Value::as_str) {
            let task = self.background_tasks.stop(task_id)?;
            return Ok(json!({ "stopped": [task] }));
        }
        Ok(json!({ "stopped": self.background_tasks.stop_all() }))
    }
}

fn spawn_background_bash_task(
    mut child: Child,
    wrapped: WrappedCommand,
    record: BackgroundTaskRecord,
    store: Arc<BackgroundTaskStore>,
    stop_rx: oneshot::Receiver<()>,
    process_group_id: Option<u32>,
) {
    tokio::spawn(async move {
        let result =
            run_background_bash_task(&mut child, wrapped, &record, stop_rx, process_group_id).await;
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

async fn run_background_bash_task(
    child: &mut Child,
    wrapped: WrappedCommand,
    record: &BackgroundTaskRecord,
    mut stop_rx: oneshot::Receiver<()>,
    process_group_id: Option<u32>,
) -> Result<Option<i32>, ToolError> {
    if let Some(parent) = record.output_path.parent() {
        fs::create_dir_all(parent).await?;
    }
    fs::write(&record.output_path, "").await?;
    if !wrapped.sandboxed {
        append_background_output(
            &record.output_path,
            BashStreamKind::Stderr,
            &unsandboxed_execution_warning(&wrapped),
        )
        .await?;
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
    let stdout_task = tokio::spawn(read_stream_chunks(
        stdout,
        BashStreamKind::Stdout,
        tx.clone(),
    ));
    let stderr_task = tokio::spawn(read_stream_chunks(stderr, BashStreamKind::Stderr, tx));

    let mut output_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&record.output_path)
        .await?;
    let mut stop_requested = false;
    loop {
        tokio::select! {
            chunk = rx.recv() => {
                let Some((stream, chunk)) = chunk else {
                    break;
                };
                if !chunk.is_empty() {
                    match stream {
                        BashStreamKind::Stdout => output_file.write_all(chunk.as_bytes()).await?,
                        BashStreamKind::Stderr => {
                            output_file.write_all(b"[stderr] ").await?;
                            output_file.write_all(chunk.as_bytes()).await?;
                        }
                    }
                }
            }
            _ = &mut stop_rx, if !stop_requested => {
                stop_requested = true;
                kill_child_process_group(process_group_id);
                let _ = child.start_kill();
                output_file.write_all(b"[stderr] background task stop requested\n").await?;
            }
        }
    }

    let status = child.wait().await?;
    stdout_task
        .await
        .map_err(|err| ToolError::ExecutionFailed(err.to_string()))??;
    stderr_task
        .await
        .map_err(|err| ToolError::ExecutionFailed(err.to_string()))??;
    if let Some(path) = wrapped.cleanup_path.as_ref() {
        let _ = fs::remove_file(path).await;
    }
    Ok(status.code())
}

async fn append_background_output(
    path: &Path,
    stream: BashStreamKind,
    chunk: &str,
) -> Result<(), ToolError> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    match stream {
        BashStreamKind::Stdout => file.write_all(chunk.as_bytes()).await?,
        BashStreamKind::Stderr => {
            file.write_all(b"[stderr] ").await?;
            file.write_all(chunk.as_bytes()).await?;
        }
    }
    Ok(())
}

async fn read_output_tail(path: &Path, max_bytes: usize) -> Result<String, ToolError> {
    let mut file = match fs::File::open(path).await {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(err) => return Err(err.into()),
    };
    let file_len = file.metadata().await?.len();
    let start = file_len.saturating_sub(max_bytes as u64);
    file.seek(SeekFrom::Start(start)).await?;
    let mut bytes = Vec::with_capacity(max_bytes.min(file_len as usize));
    file.read_to_end(&mut bytes).await?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

async fn read_stream_chunks<R>(
    reader: R,
    stream: BashStreamKind,
    tx: mpsc::Sender<(BashStreamKind, String)>,
) -> Result<(), ToolError>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut reader = reader;
    let mut buffer = [0_u8; 4096];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let chunk = String::from_utf8_lossy(&buffer[..read]).into_owned();
        if tx.send((stream, chunk)).await.is_err() {
            break;
        }
    }
    Ok(())
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

fn kill_child_process_group(child_id: Option<u32>) {
    #[cfg(unix)]
    {
        let Some(child_id) = child_id else {
            return;
        };
        let process_group = -(child_id as libc::pid_t);
        // Best-effort cancellation: the direct child may have already exited,
        // but killing the group stops shell descendants that still hold pipes.
        unsafe {
            libc::kill(process_group, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child_id;
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
mod tests {
    use std::collections::HashMap;
    use std::env;
    use std::path::Path;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use std::time::Duration;

    use serde_json::{Value, json};
    use tempfile::tempdir;

    use super::{
        BackgroundTaskListTool, BackgroundTaskStatus, BackgroundTaskStatusTool,
        BackgroundTaskStopTool, BackgroundTaskStore, BashCommandInput, BashSandboxPermissions,
        BashStreamKind, BashTool, append_aggregated_bash_output, command_env_for_wrapped,
        read_output_tail, sandbox_command_env, sandbox_output_hint, unsandboxed_execution_warning,
    };
    use crate::sandbox::{SandboxManager, WrappedCommand};
    use crate::tool::{Tool, ToolCallContext, ToolOutputStream, ToolProgressEvent};
    use crate::tool_result::model_preview_bash_output;

    #[test]
    fn parses_legacy_shell_payload() {
        let input = BashCommandInput::from_value(json!({
            "command": "cargo test",
            "allow_net": true
        }))
        .expect("legacy payload");

        assert_eq!(input.command.as_deref(), Some("cargo test"));
        assert!(input.allow_net);
        assert!(!input.run_in_background);
        assert_eq!(input.summary(), "cargo test");
    }

    #[test]
    fn parses_structured_payload() {
        let input = BashCommandInput::from_value(json!({
            "program": "cargo",
            "args": ["check", "--workspace"],
            "cwd": "/tmp/workspace",
            "env": { "RUST_LOG": "debug" },
            "allow_net": false
        }))
        .expect("structured payload");

        assert_eq!(input.program.as_deref(), Some("cargo"));
        assert_eq!(
            input.args,
            vec!["check".to_string(), "--workspace".to_string()]
        );
        assert_eq!(input.cwd.as_deref(), Some("/tmp/workspace"));
        assert_eq!(input.env.get("RUST_LOG").map(String::as_str), Some("debug"));
        assert!(!input.run_in_background);
        assert_eq!(input.summary(), "cargo check --workspace");
    }

    #[test]
    fn parses_background_payload() {
        let input = BashCommandInput::from_value(json!({
            "program": "cargo",
            "args": ["test"],
            "run_in_background": true
        }))
        .expect("background payload");

        assert!(input.run_in_background);
        assert_eq!(input.summary(), "cargo test");
    }

    #[test]
    fn parses_codex_style_escalated_sandbox_request() {
        let input = BashCommandInput::from_value(json!({
            "program": "cargo",
            "args": ["check"],
            "sandbox_permissions": "require_escalated",
            "justification": "Do you want to run cargo check outside the sandbox?",
            "prefix_rule": ["cargo", "check"]
        }))
        .expect("escalated payload");

        assert_eq!(
            input.sandbox_permissions,
            BashSandboxPermissions::RequireEscalated
        );
        assert_eq!(
            input.justification.as_deref(),
            Some("Do you want to run cargo check outside the sandbox?")
        );
        assert_eq!(input.approval_prefix().as_deref(), Some("cargo check"));
        assert!(!input.is_read_only());
    }

    #[test]
    fn bash_tool_schema_guides_command_discipline() {
        let temp = tempdir().expect("tempdir");
        let tool = BashTool {
            sandbox: Arc::new(
                SandboxManager::new_for_rara_dir(temp.path().join(".rara")).expect("sandbox"),
            ),
            background_tasks: Arc::new(
                BackgroundTaskStore::new(temp.path().join(".rara/background-tasks"))
                    .expect("background task store"),
            ),
            base_env: Arc::new(HashMap::new()),
            sandbox_network_access: false,
        };

        let description = tool.description();
        assert!(description.contains("Prefer dedicated RARA tools"));
        assert!(description.contains("apply_patch"));
        assert!(description.contains("cwd field"));
        assert!(description.contains("newline-separated command chaining"));
        assert!(description.contains("Commands must be non-interactive"));
        assert!(description.contains("git commit -m"));
        assert!(description.contains("require_escalated"));
        assert!(description.contains("background_task_status"));

        let schema = tool.input_schema().to_string();
        assert!(schema.contains("Prefer program+args"));
        assert!(schema.contains("direct file tools"));
        assert!(schema.contains("never bare git commit"));
        assert!(schema.contains("prefer this over prepending cd"));
        assert!(schema.contains("sandbox failure evidence"));
        assert!(schema.contains("Do not suggest broad prefixes"));
    }

    #[test]
    fn background_task_tool_descriptions_point_to_run_in_background() {
        let temp = tempdir().expect("tempdir");
        let background_tasks = Arc::new(
            BackgroundTaskStore::new(temp.path().join(".rara/background-tasks"))
                .expect("background task store"),
        );
        let list = BackgroundTaskListTool {
            background_tasks: background_tasks.clone(),
        };
        let status = BackgroundTaskStatusTool {
            background_tasks: background_tasks.clone(),
        };
        let stop = BackgroundTaskStopTool { background_tasks };

        assert!(list.description().contains("run_in_background"));
        assert!(list.description().contains("duplicate long-running work"));
        assert!(status.description().contains("run_in_background"));
        assert!(stop.description().contains("task_id is omitted"));
    }

    #[test]
    fn escalated_sandbox_request_allows_missing_justification() {
        let input = BashCommandInput::from_value(json!({
            "program": "cargo",
            "args": ["check"],
            "sandbox_permissions": "require_escalated"
        }))
        .expect("escalated payload");

        assert_eq!(
            input.sandbox_permissions,
            BashSandboxPermissions::RequireEscalated
        );
        assert!(input.justification.is_none());
        assert!(!input.is_read_only());
    }

    #[test]
    fn classifies_read_only_commands_for_approval_policy() {
        for command in [
            "git status --short",
            "git diff -- src/tools/bash.rs",
            "git log --oneline -n 5",
            "rg -n read_only src",
            "find src -name '*.rs'",
            "sed -n '1,20p' src/tools/bash.rs",
            "cat Cargo.toml | grep '^name'",
            "docker inspect rara-dev",
            "pyright --outputjson",
        ] {
            let input =
                BashCommandInput::from_value(json!({ "command": command })).expect("bash payload");
            assert!(input.is_read_only(), "{command} should be read-only");
        }
    }

    #[test]
    fn keeps_write_network_background_and_complex_commands_under_approval() {
        for payload in [
            json!({ "command": "git push origin main" }),
            json!({ "command": "rm -rf target" }),
            json!({ "command": "sed -i '' 's/a/b/' Cargo.toml" }),
            json!({ "command": "find . -name '*.tmp' -delete" }),
            json!({ "command": "cat Cargo.toml > /tmp/out" }),
            json!({ "command": "git status", "allow_net": true }),
            json!({ "command": "rg TODO", "run_in_background": true }),
            json!({ "program": "rg", "args": ["TODO"], "env": { "PATH": "/tmp/bin" } }),
        ] {
            let input = BashCommandInput::from_value(payload).expect("bash payload");
            assert!(
                !input.is_read_only(),
                "{} should require approval",
                input.summary()
            );
        }
    }

    #[test]
    fn classifies_structured_read_only_programs() {
        let input = BashCommandInput::from_value(json!({
            "program": "/usr/bin/git",
            "args": ["status", "--short"]
        }))
        .expect("structured payload");

        assert!(input.is_read_only());
    }

    #[test]
    fn derives_and_matches_codex_style_approval_prefix() {
        let input = BashCommandInput::from_value(json!({
            "command": "git push origin main"
        }))
        .expect("bash payload");

        assert_eq!(input.approval_prefix().as_deref(), Some("git push"));
        assert!(input.matches_approval_prefix("git push"));
        assert!(!input.matches_approval_prefix("git pull"));
    }

    #[test]
    fn approval_prefix_matching_normalizes_program_paths() {
        let shell_input = BashCommandInput::from_value(json!({
            "command": "/usr/bin/git push origin main"
        }))
        .expect("shell payload");
        assert_eq!(shell_input.approval_prefix().as_deref(), Some("git push"));
        assert!(shell_input.matches_approval_prefix("git push"));

        let structured_input = BashCommandInput::from_value(json!({
            "program": "/usr/bin/git",
            "args": ["push", "origin", "main"]
        }))
        .expect("structured payload");
        assert_eq!(
            structured_input.approval_prefix().as_deref(),
            Some("git push")
        );
        assert!(structured_input.matches_approval_prefix("git push"));
    }

    #[test]
    fn approval_prefix_skips_known_global_options() {
        let input = BashCommandInput::from_value(json!({
            "command": "git --no-pager push origin main"
        }))
        .expect("shell payload");

        assert_eq!(input.approval_prefix().as_deref(), Some("git push"));
        assert!(input.matches_approval_prefix("git push"));
    }

    #[test]
    fn sandbox_command_env_defaults_home_and_xdg_roots() {
        let sandbox_home = Path::new("/tmp/rara-test-home");
        let base_env = HashMap::from([("PATH".to_string(), "/custom/bin:/usr/bin".to_string())]);
        let env_map = sandbox_command_env(sandbox_home, &base_env, &HashMap::new(), true);

        assert_eq!(
            env_map.get("HOME").map(String::as_str),
            Some("/tmp/rara-test-home")
        );
        assert_eq!(
            env_map.get("XDG_CONFIG_HOME").map(String::as_str),
            Some("/tmp/rara-test-home/.config")
        );
        assert_eq!(
            env_map.get("XDG_CACHE_HOME").map(String::as_str),
            Some("/tmp/rara-test-home/.cache")
        );
        assert_eq!(
            env_map.get("PATH").map(String::as_str),
            Some("/custom/bin:/usr/bin")
        );
    }

    #[test]
    fn sandbox_command_env_keeps_explicit_overrides() {
        let sandbox_home = Path::new("/tmp/rara-test-home");
        let env_map = sandbox_command_env(
            sandbox_home,
            &HashMap::from([("PATH".to_string(), "/snapshot/bin".to_string())]),
            &HashMap::from([
                ("HOME".to_string(), "/custom/home".to_string()),
                (
                    "XDG_CACHE_HOME".to_string(),
                    "/custom/home/.cache".to_string(),
                ),
                ("PATH".to_string(), "/override/bin".to_string()),
            ]),
            true,
        );

        assert_eq!(
            env_map.get("HOME").map(String::as_str),
            Some("/custom/home")
        );
        assert_eq!(
            env_map.get("XDG_CACHE_HOME").map(String::as_str),
            Some("/custom/home/.cache")
        );
        assert_eq!(
            env_map.get("XDG_CONFIG_HOME").map(String::as_str),
            Some("/tmp/rara-test-home/.config")
        );
        assert_eq!(
            env_map.get("PATH").map(String::as_str),
            Some("/override/bin")
        );
    }

    #[test]
    fn sandbox_command_env_falls_back_to_process_path_when_snapshot_path_is_missing() {
        let sandbox_home = Path::new("/tmp/rara-test-home");
        let env_map = sandbox_command_env(
            sandbox_home,
            &HashMap::from([("PATH".to_string(), String::new())]),
            &HashMap::new(),
            true,
        );

        assert!(
            env_map.get("PATH").is_some_and(|path| !path.is_empty()),
            "sandbox env must keep a usable PATH after env_clear"
        );
    }

    #[test]
    fn sandbox_command_env_marks_disabled_network() {
        let env_map = sandbox_command_env(
            Path::new("/tmp/rara-test-home"),
            &HashMap::new(),
            &HashMap::new(),
            false,
        );

        assert_eq!(
            env_map
                .get("RARA_SANDBOX_NETWORK_DISABLED")
                .map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn sandbox_output_hint_explains_blocked_shell_paths() {
        let hint = sandbox_output_hint("sandbox-exec: /bin/sed: Operation not permitted")
            .expect("sandbox hint");

        assert!(hint.contains("Prefer direct file tools"));
        assert!(hint.contains("replace_lines"));
    }

    #[test]
    fn direct_wrapped_command_keeps_caller_environment_overrides_only() {
        let wrapped = WrappedCommand {
            program: "sh".to_string(),
            args: vec!["-c".to_string(), "pwd".to_string()],
            cleanup_path: None,
            sandboxed: false,
            sandbox_backend: "direct".to_string(),
            sandbox_home: None,
            network_access: true,
        };
        let env_map = command_env_for_wrapped(
            &wrapped,
            &HashMap::from([("PATH".to_string(), "/snapshot/bin".to_string())]),
            &HashMap::from([("HOME".to_string(), "/real/home".to_string())]),
        )
        .expect("direct env");

        assert_eq!(env_map.get("HOME").map(String::as_str), Some("/real/home"));
        assert_eq!(
            env_map.get("PATH").map(String::as_str),
            Some("/snapshot/bin")
        );
        assert!(
            !env_map.contains_key("XDG_CONFIG_HOME"),
            "direct fallback should not apply sandbox-only XDG roots"
        );
    }

    #[test]
    fn unsandboxed_warning_names_the_backend() {
        let wrapped = WrappedCommand {
            program: "sh".to_string(),
            args: vec!["-c".to_string(), "pwd".to_string()],
            cleanup_path: None,
            sandboxed: false,
            sandbox_backend: "direct".to_string(),
            sandbox_home: None,
            network_access: true,
        };

        let warning = unsandboxed_execution_warning(&wrapped);

        assert!(warning.contains("without sandbox isolation"));
        assert!(warning.contains("direct"));
    }

    #[tokio::test]
    async fn escalated_sandbox_request_runs_directly_after_approval() {
        let temp = tempdir().expect("tempdir");
        let sandbox = SandboxManager::new_for_rara_dir(temp.path().join(".rara")).expect("sandbox");
        let tool = BashTool {
            sandbox: Arc::new(sandbox),
            background_tasks: Arc::new(
                BackgroundTaskStore::new(temp.path().join(".rara/background-tasks"))
                    .expect("background task store"),
            ),
            base_env: Arc::new(HashMap::new()),
            sandbox_network_access: false,
        };

        let result = tool
            .call(json!({
                "program": "sh",
                "args": ["-c", "printf direct"],
                "sandbox_permissions": "require_escalated",
                "justification": "Do you want to run this shell outside the sandbox?"
            }))
            .await
            .expect("bash result");

        assert_eq!(
            result.get("sandboxed").and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            result.get("sandbox_backend").and_then(Value::as_str),
            Some("direct")
        );
        assert_eq!(result.get("stdout").and_then(Value::as_str), Some("direct"));
        let aggregated_output = result
            .get("aggregated_output")
            .and_then(Value::as_str)
            .expect("aggregated output");
        assert!(aggregated_output.contains("direct"));
        assert!(aggregated_output.contains("without sandbox isolation"));
        assert!(result.get("duration_ms").and_then(Value::as_u64).is_some());
        assert!(
            result
                .get("stderr")
                .and_then(Value::as_str)
                .is_some_and(|stderr| stderr.contains("without sandbox isolation"))
        );
    }

    #[tokio::test]
    async fn streaming_call_reports_stdout_and_stderr_chunks() {
        let temp = tempdir().expect("tempdir");
        let sandbox = SandboxManager::new_for_rara_dir(temp.path().join(".rara")).expect("sandbox");
        let Ok(wrapped) = sandbox.wrap_exec_command(
            "/bin/sh",
            &[
                "-c".to_string(),
                "printf 'out\\n'; printf 'err\\n' >&2".to_string(),
            ],
            temp.path().to_string_lossy().as_ref(),
            false,
        ) else {
            return;
        };
        if !binary_exists(&wrapped.program) {
            return;
        }
        // Streaming through some sandbox backends (e.g. macOS seatbelt)
        // is not supported; skip this test under sandboxed execution.
        if wrapped.sandboxed {
            return;
        }
        let tool = BashTool {
            sandbox: Arc::new(sandbox),
            background_tasks: Arc::new(
                BackgroundTaskStore::new(temp.path().join(".rara/background-tasks"))
                    .expect("background task store"),
            ),
            base_env: Arc::new(HashMap::new()),
            sandbox_network_access: false,
        };
        let mut events = Vec::new();
        let result = tool
            .call_with_events(
                json!({
                    "program": "/bin/sh",
                    "args": ["-c", "printf 'out\\n'; printf 'err\\n' >&2"],
                }),
                &mut |event| events.push(event),
            )
            .await
            .expect("bash result");

        assert!(
            !events.is_empty(),
            "expected streamed events, got result: {result}"
        );
        assert!(events.iter().any(|event| matches!(
            event,
            ToolProgressEvent::Output {
                stream: ToolOutputStream::Stdout | ToolOutputStream::Stderr,
                ..
            }
        )));
        assert_eq!(
            result.get("live_streamed").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            result.get("sandboxed").and_then(Value::as_bool),
            Some(wrapped.sandboxed)
        );
        assert_eq!(
            result.get("sandbox_backend").and_then(Value::as_str),
            Some(wrapped.sandbox_backend.as_str())
        );
        let aggregated_output = result
            .get("aggregated_output")
            .and_then(Value::as_str)
            .expect("aggregated output");
        assert!(aggregated_output.contains("out"));
        assert!(aggregated_output.contains("[stderr] err"));
        let model_preview_output = result
            .get("model_preview_output")
            .and_then(Value::as_str)
            .expect("model preview output");
        assert!(model_preview_output.contains("out"));
        assert!(model_preview_output.contains("[stderr] err"));
        assert!(result.get("duration_ms").and_then(Value::as_u64).is_some());
    }

    #[tokio::test]
    async fn foreground_bash_can_be_cancelled() {
        let temp = tempdir().expect("tempdir");
        let sandbox = SandboxManager::new_for_rara_dir(temp.path().join(".rara")).expect("sandbox");
        let tool = BashTool {
            sandbox: Arc::new(sandbox),
            background_tasks: Arc::new(
                BackgroundTaskStore::new(temp.path().join(".rara/background-tasks"))
                    .expect("background task store"),
            ),
            base_env: Arc::new(HashMap::new()),
            sandbox_network_access: false,
        };
        let cancellation = Arc::new(AtomicBool::new(false));
        let cancellation_for_task = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancellation_for_task.store(true, Ordering::SeqCst);
        });
        let mut events = Vec::new();

        let err = tool
            .call_with_context_events(
                json!({
                    "program": "sh",
                    "args": ["-c", "sleep 30 & wait"],
                    "sandbox_permissions": "require_escalated"
                }),
                ToolCallContext::default().with_cancellation(cancellation),
                &mut |event| events.push(event),
            )
            .await
            .expect_err("bash should be cancelled");

        assert!(err.to_string().contains("cancelled by user"));
        assert!(events.iter().any(|event| matches!(
            event,
            ToolProgressEvent::Output {
                stream: ToolOutputStream::Stderr,
                chunk,
            } if chunk.contains("cancelled by user")
        )));
    }

    #[test]
    fn aggregated_stderr_prefixes_only_line_boundaries() {
        let mut output = String::new();
        let mut last_stream = None;
        append_aggregated_bash_output(
            &mut output,
            &mut last_stream,
            BashStreamKind::Stderr,
            "partial",
        );
        append_aggregated_bash_output(
            &mut output,
            &mut last_stream,
            BashStreamKind::Stderr,
            "-line\nnext",
        );
        append_aggregated_bash_output(
            &mut output,
            &mut last_stream,
            BashStreamKind::Stderr,
            "-line\n",
        );

        assert_eq!(output, "[stderr] partial-line\n[stderr] next-line\n");
    }

    #[test]
    fn aggregated_stderr_starts_on_new_line_after_stdout() {
        let mut output = String::new();
        let mut last_stream = None;
        append_aggregated_bash_output(
            &mut output,
            &mut last_stream,
            BashStreamKind::Stdout,
            "stdout-without-newline",
        );
        append_aggregated_bash_output(
            &mut output,
            &mut last_stream,
            BashStreamKind::Stderr,
            "stderr-line\n",
        );

        assert_eq!(output, "stdout-without-newline\n[stderr] stderr-line\n");
    }

    #[test]
    fn model_preview_bash_output_preserves_error_tail() {
        let output = format!("head\n{}tail-error\n", "middle\n".repeat(2_000));

        let preview = model_preview_bash_output(&output, Some(1));

        assert!(preview.contains("head"));
        assert!(preview.contains("tail-error"));
        assert!(preview.contains("chars truncated from middle"));
    }

    #[tokio::test]
    async fn background_call_returns_task_and_status_reads_output() {
        let temp = tempdir().expect("tempdir");
        let sandbox = SandboxManager::new_for_rara_dir(temp.path().join(".rara")).expect("sandbox");
        let Ok(wrapped) = sandbox.wrap_exec_command(
            "sh",
            &["-c".to_string(), "printf 'background-out\\n'".to_string()],
            temp.path().to_string_lossy().as_ref(),
            false,
        ) else {
            return;
        };
        if !binary_exists(&wrapped.program) {
            return;
        }

        let background_tasks = Arc::new(
            BackgroundTaskStore::new(temp.path().join(".rara/background-tasks"))
                .expect("background task store"),
        );
        let tool = BashTool {
            sandbox: Arc::new(sandbox),
            background_tasks: background_tasks.clone(),
            base_env: Arc::new(HashMap::new()),
            sandbox_network_access: false,
        };
        let status_tool = BackgroundTaskStatusTool {
            background_tasks: background_tasks.clone(),
        };

        let started = tool
            .call(json!({
                "program": "sh",
                "args": ["-c", "printf 'background-out\\n'"],
                "run_in_background": true,
            }))
            .await
            .expect("background start");
        let task_id = started
            .get("background_task_id")
            .and_then(Value::as_str)
            .expect("task id");
        assert_eq!(started.get("exit_code"), Some(&Value::Null));
        assert_eq!(
            started.get("status"),
            Some(&json!(BackgroundTaskStatus::Running))
        );
        assert_eq!(
            started.get("network_access").and_then(Value::as_bool),
            Some(wrapped.network_access)
        );

        let mut last = Value::Null;
        for _ in 0..50 {
            last = status_tool
                .call(json!({ "task_id": task_id, "tail_bytes": 4096 }))
                .await
                .expect("background status");
            if last.get("status") != Some(&json!(BackgroundTaskStatus::Running)) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        assert_ne!(
            last.get("status"),
            Some(&json!(BackgroundTaskStatus::Running))
        );
        assert!(last.get("output_path").and_then(Value::as_str).is_some());
        assert_eq!(
            last.get("network_access").and_then(Value::as_bool),
            Some(wrapped.network_access)
        );
    }

    #[tokio::test]
    async fn background_tasks_can_be_listed_and_stopped_without_count_limit() {
        let temp = tempdir().expect("tempdir");
        let sandbox = SandboxManager::new_for_rara_dir(temp.path().join(".rara")).expect("sandbox");
        let Ok(wrapped) = sandbox.wrap_exec_command(
            "sh",
            &["-c".to_string(), "sleep 30".to_string()],
            temp.path().to_string_lossy().as_ref(),
            false,
        ) else {
            return;
        };
        if !binary_exists(&wrapped.program) {
            return;
        }

        let background_tasks = Arc::new(
            BackgroundTaskStore::new(temp.path().join(".rara/background-tasks"))
                .expect("background task store"),
        );
        let tool = BashTool {
            sandbox: Arc::new(sandbox),
            background_tasks: background_tasks.clone(),
            base_env: Arc::new(HashMap::new()),
            sandbox_network_access: false,
        };
        let list_tool = BackgroundTaskListTool {
            background_tasks: background_tasks.clone(),
        };
        let stop_tool = BackgroundTaskStopTool {
            background_tasks: background_tasks.clone(),
        };

        let started = tool
            .call(json!({
                "program": "sh",
                "args": ["-c", "sleep 30"],
                "run_in_background": true,
            }))
            .await
            .expect("background start");
        let task_id = started
            .get("background_task_id")
            .and_then(Value::as_str)
            .expect("task id")
            .to_string();

        let listed = list_tool.call(json!({})).await.expect("list tasks");
        assert_eq!(
            listed.get("tasks").and_then(Value::as_array).map(Vec::len),
            Some(1)
        );
        assert_eq!(
            listed
                .pointer("/tasks/0/network_access")
                .and_then(Value::as_bool),
            Some(wrapped.network_access)
        );

        let stopped = stop_tool
            .call(json!({ "task_id": task_id }))
            .await
            .expect("stop task");
        assert_eq!(
            stopped.pointer("/stopped/0/status"),
            Some(&json!(BackgroundTaskStatus::Killed))
        );
        assert_eq!(
            stopped
                .pointer("/stopped/0/network_access")
                .and_then(Value::as_bool),
            Some(wrapped.network_access)
        );
    }

    #[tokio::test]
    async fn read_output_tail_returns_only_requested_suffix() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("task.log");
        tokio::fs::write(&path, b"0123456789tail")
            .await
            .expect("write log");

        let output = read_output_tail(&path, 4).await.expect("tail");

        assert_eq!(output, "tail");
    }

    #[tokio::test]
    async fn read_output_tail_missing_file_is_empty() {
        let temp = tempdir().expect("tempdir");

        let output = read_output_tail(&temp.path().join("missing.log"), 4)
            .await
            .expect("missing tail");

        assert_eq!(output, "");
    }

    fn binary_exists(program: &str) -> bool {
        let program_path = Path::new(program);
        if program_path.components().count() > 1 {
            return program_path.exists();
        }

        env::var_os("PATH")
            .map(|paths| env::split_paths(&paths).any(|dir| dir.join(program).exists()))
            .unwrap_or(false)
    }
}
