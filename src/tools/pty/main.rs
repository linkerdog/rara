use std::collections::HashMap;
use std::env;
use std::fs::OpenOptions;
use std::io::{Read, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use portable_pty::{Child, CommandBuilder, NativePtySystem, PtySize, PtySystem};
use rara_tool_macros::tool_spec;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use uuid::Uuid;

use crate::sandbox::{SandboxManager, WrappedCommand, sandbox_failure_hint};
use crate::tool::{Tool, ToolError};

const PTY_START_QUICK_COMPLETION_TIMEOUT: Duration = Duration::from_millis(750);
const PTY_START_QUICK_COMPLETION_POLL: Duration = Duration::from_millis(25);

pub struct PtyStartTool {
    pub sessions: Arc<PtySessionStore>,
    pub sandbox: Arc<SandboxManager>,
    pub base_env: Arc<HashMap<String, String>>,
    pub sandbox_network_access: Arc<AtomicBool>,
}

pub struct PtyReadTool {
    pub sessions: Arc<PtySessionStore>,
}

pub struct PtyListTool {
    pub sessions: Arc<PtySessionStore>,
}

pub struct PtyStatusTool {
    pub sessions: Arc<PtySessionStore>,
}

pub struct PtyWriteTool {
    pub sessions: Arc<PtySessionStore>,
}

pub struct PtyKillTool {
    pub sessions: Arc<PtySessionStore>,
}

pub struct PtyStopTool {
    pub sessions: Arc<PtySessionStore>,
}

pub struct PtySessionStore {
    dir: PathBuf,
    sessions: Mutex<HashMap<String, PtySessionRecord>>,
}

struct PtySessionRecord {
    id: String,
    command: String,
    output_path: PathBuf,
    sandboxed: bool,
    sandbox_backend: String,
    network_access: bool,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Arc<Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
    child_pid: Option<u32>,
    status: Arc<Mutex<PtySessionStatus>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PtySessionStatus {
    Running,
    Killing,
    Completed,
    Killed,
}

impl PtySessionStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Killing => "killing",
            Self::Completed => "completed",
            Self::Killed => "killed",
        }
    }
}

impl PtySessionStore {
    pub fn new(dir: PathBuf) -> Result<Self, ToolError> {
        std::fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            sessions: Mutex::new(HashMap::new()),
        })
    }

    fn start(
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
        };
        let snapshot = record.snapshot();
        self.sessions
            .lock()
            .expect("pty session store lock")
            .insert(id, record);
        Ok(snapshot)
    }

    fn get(&self, id: &str) -> Option<PtySessionSnapshot> {
        self.sessions
            .lock()
            .expect("pty session store lock")
            .get(id)
            .map(PtySessionRecord::snapshot)
    }

    async fn wait_for_quick_completion(&self, id: &str, timeout: Duration) -> PtySessionSnapshot {
        let Some(deadline) = Instant::now().checked_add(timeout) else {
            // Timeout too large — return whatever snapshot we have now.
            return self
                .get(id)
                .unwrap_or_else(|| PtySessionSnapshot::missing(id));
        };

        // Fetch the session handle once so we can poll status without
        // calling self.get(id) (which clones the whole snapshot) on every
        // iteration.  We still refresh the full snapshot on completion.
        let mut snapshot = match self.get(id) {
            Some(snap) => snap,
            None => return PtySessionSnapshot::missing(id),
        };

        while matches!(snapshot.status, PtySessionStatus::Running) {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let remaining = deadline - now;
            let sleep_duration = remaining.min(PTY_START_QUICK_COMPLETION_POLL);
            tokio::time::sleep(sleep_duration).await;

            match self.get(id) {
                Some(next) => snapshot = next,
                None => break,
            }
        }
        snapshot
    }

    fn list(&self) -> Vec<PtySessionSnapshot> {
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

    fn write(&self, id: &str, input: &str) -> Result<PtySessionSnapshot, ToolError> {
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

    fn kill(&self, id: &str) -> Result<PtySessionSnapshot, ToolError> {
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
        if should_kill {
            if let Err(err) =
                kill_pty_child(&mut **child.lock().expect("pty child lock"), child_pid)
            {
                restore_running_after_failed_kill(&status);
                return Err(ToolError::ExecutionFailed(format!(
                    "kill pty session: {err}"
                )));
            }
        }
        Ok(snapshot)
    }

    fn kill_all(&self) -> Vec<PtySessionSnapshot> {
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

struct PtySessionSnapshot {
    id: String,
    command: String,
    output_path: PathBuf,
    sandboxed: bool,
    sandbox_backend: String,
    network_access: bool,
    status: PtySessionStatus,
}

impl PtySessionSnapshot {
    fn missing(id: &str) -> Self {
        Self {
            id: id.to_string(),
            command: String::new(),
            output_path: PathBuf::new(),
            sandboxed: false,
            sandbox_backend: String::new(),
            network_access: false,
            status: PtySessionStatus::Completed,
        }
    }
}

impl PtySessionRecord {
    fn snapshot(&self) -> PtySessionSnapshot {
        PtySessionSnapshot {
            id: self.id.clone(),
            command: self.command.clone(),
            output_path: self.output_path.clone(),
            sandboxed: self.sandboxed,
            sandbox_backend: self.sandbox_backend.clone(),
            network_access: self.network_access,
            status: *self.status.lock().expect("pty status lock"),
        }
    }
}

impl PtySessionSnapshot {
    fn metadata_json(self) -> Value {
        json!({
            "session_id": self.id,
            "command": self.command,
            "status": self.status.as_str(),
            "output_path": self.output_path,
            "sandboxed": self.sandboxed,
            "sandbox_backend": self.sandbox_backend,
            "network_access": self.network_access,
        })
    }

    async fn into_json(self, tail_bytes: usize) -> Result<Value, ToolError> {
        let output = read_output_tail(&self.output_path, tail_bytes).await?;
        Ok(json!({
            "session_id": self.id,
            "command": self.command,
            "status": self.status.as_str(),
            "output_path": self.output_path,
            "sandboxed": self.sandboxed,
            "sandbox_backend": self.sandbox_backend,
            "network_access": self.network_access,
            "output": output,
        }))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PtyCommandInput {
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
    #[serde(default = "default_rows")]
    pub rows: u16,
    #[serde(default = "default_cols")]
    pub cols: u16,
}

fn default_rows() -> u16 {
    24
}
fn default_cols() -> u16 {
    120
}

impl PtyCommandInput {
    pub fn from_value(input: Value) -> Result<Self, ToolError> {
        let parsed: Self = serde_json::from_value(input)
            .map_err(|err| ToolError::InvalidInput(format!("pty payload: {err}")))?;
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
                "pty payload requires either command or program".into(),
            ));
        }
        if self.rows == 0 {
            return Err(ToolError::InvalidInput("rows must be >= 1".into()));
        }
        if self.cols == 0 {
            return Err(ToolError::InvalidInput("cols must be >= 1".into()));
        }
        Ok(())
    }

    pub fn working_dir(&self) -> String {
        match self.cwd.as_ref() {
            Some(cwd) if !cwd.trim().is_empty() => cwd.clone(),
            _ => env::current_dir()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| ".".to_string()),
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
}
#[tool_spec(
    name = "pty_start",
    description = "Start an interactive PTY session only for commands that need terminal input, terminal control, or an interactive program. For ordinary non-interactive commands, use bash instead. Prefer dedicated RARA tools for file search, file reads, and file edits. Use the cwd field instead of prepending cd. PTY sandboxing is platform-dependent and best-effort; with the macOS seatbelt backend, PTY commands currently run directly because sandbox-exec does not preserve interactive PTY stdin reliably. Treat allow_net as a network-access toggle, not a sandbox guarantee. Inspect or stop sessions with pty_status, pty_list, and pty_stop.",
    input_schema = {
        "type": "object",
        "properties": {
            "command": {
                "type": "string",
                "description": "Shell command to run inside a PTY. Use PTY only for interactive commands; use bash for ordinary non-interactive commands."
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
                "description": "Optional working directory. Defaults to the current turn cwd; prefer this over prepending cd to a command."
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
        let request = PtyCommandInput::from_value(input)?;
        let cwd = request.working_dir();
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
fn command_env_for_wrapped(
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
    let needs_path = env_map.get("PATH").map_or(true, |value| value.is_empty());
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

fn kill_pty_child(child: &mut dyn Child, child_pid: Option<u32>) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    if let Some(child_pid) = child_pid {
        let process_group_result = kill_child_process_group(child_pid);
        let child_result = child.kill();
        return match (process_group_result, child_result) {
            (Err(group_err), _) => Err(group_err),
            (Ok(()), Ok(())) => Ok(()),
            (Ok(()), Err(err)) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            (Ok(()), Err(err)) => Err(err),
        };
    }

    #[cfg(not(unix))]
    let _ = child_pid;

    child.kill()
}

fn restore_running_after_failed_kill(status: &Mutex<PtySessionStatus>) {
    let mut status = status.lock().expect("pty status lock");
    if matches!(*status, PtySessionStatus::Killing) {
        *status = PtySessionStatus::Running;
    }
}

#[cfg(unix)]
fn kill_child_process_group(child_pid: u32) -> Result<(), std::io::Error> {
    use nix::errno::Errno;
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::{Pid, getpgid};

    let child_pid = Pid::from_raw(child_pid as i32);
    let process_group_id = match getpgid(Some(child_pid)) {
        Ok(process_group_id) => process_group_id,
        Err(Errno::ESRCH) => return Ok(()),
        Err(err) => return Err(std::io::Error::from_raw_os_error(err as i32)),
    };

    match killpg(process_group_id, Signal::SIGKILL) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(err) => Err(std::io::Error::from_raw_os_error(err as i32)),
    }
}

async fn read_output_tail(path: &Path, max_bytes: usize) -> Result<String, ToolError> {
    let mut file = match tokio::fs::File::open(path).await {
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

fn parse_pty_dimension(value: Option<&Value>, default: u16, name: &str) -> Result<u16, ToolError> {
    let Some(value) = value else {
        return Ok(default);
    };
    let Some(value) = value.as_u64() else {
        return Err(ToolError::InvalidInput(format!(
            "{name} must be an integer"
        )));
    };
    if value == 0 || value > u16::MAX as u64 {
        return Err(ToolError::InvalidInput(format!(
            "{name} must be between 1 and {}",
            u16::MAX
        )));
    }
    Ok(value as u16)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::sandbox::WrappedCommand;

    #[tokio::test]
    async fn read_output_tail_returns_only_requested_suffix() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("pty.log");
        tokio::fs::write(&path, b"0123456789tail")
            .await
            .expect("write log");

        let output = read_output_tail(&path, 4).await.expect("tail");

        assert_eq!(output, "tail");
    }

    #[test]
    fn parse_pty_dimension_rejects_overflowing_values() {
        let value = json!(u16::MAX as u64 + 1);

        let err = parse_pty_dimension(Some(&value), 24, "rows").expect_err("overflow rejected");

        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[test]
    fn pty_tool_schema_guides_interactive_command_discipline() {
        let temp = tempdir().expect("tempdir");
        let sessions = Arc::new(PtySessionStore::new(temp.path().join("pty")).expect("pty store"));
        let start = PtyStartTool {
            sessions: sessions.clone(),
            sandbox: Arc::new(
                SandboxManager::new_for_rara_dir(temp.path().join(".rara")).expect("sandbox"),
            ),
            base_env: Arc::new(HashMap::new()),
            sandbox_network_access: Arc::new(AtomicBool::new(false)),
        };
        let list = PtyListTool {
            sessions: sessions.clone(),
        };
        let status = PtyStatusTool {
            sessions: sessions.clone(),
        };
        let stop = PtyStopTool { sessions };

        let description = start.description();
        assert!(description.contains("interactive PTY session only"));
        assert!(description.contains("use bash instead"));
        assert!(description.contains("Prefer dedicated RARA tools"));
        assert!(description.contains("cwd field"));
        assert!(description.contains("sandboxing is platform-dependent"));
        assert!(description.contains("macOS seatbelt backend"));
        assert!(description.contains("allow_net as a network-access toggle"));
        assert!(description.contains("pty_status"));
        assert!(description.contains("pty_list"));
        assert!(description.contains("pty_stop"));

        let schema = start.input_schema().to_string();
        assert!(schema.contains("Use PTY only for interactive commands"));
        assert!(schema.contains("use bash for ordinary non-interactive commands"));
        assert!(schema.contains("prefer this over prepending cd"));
        assert!(list.description().contains("duplicate interactive work"));
        assert!(status.description().contains("pty_start"));
        assert!(stop.description().contains("session_id is omitted"));
    }

    #[test]
    fn sandboxed_pty_env_falls_back_to_process_path_when_snapshot_path_is_missing() {
        let temp = tempdir().expect("tempdir");
        let wrapped = WrappedCommand {
            program: "bwrap".to_string(),
            args: vec!["--version".to_string()],
            cleanup_path: None,
            sandboxed: true,
            sandbox_backend: "linux-bubblewrap".to_string(),
            sandbox_home: Some(temp.path().join("home")),
            network_access: false,
        };
        let env_map = command_env_for_wrapped(
            &wrapped,
            &HashMap::from([("PATH".to_string(), String::new())]),
            &HashMap::new(),
        )
        .expect("pty env");

        assert!(
            env_map.get("PATH").is_some_and(|path| !path.is_empty()),
            "sandboxed PTY env must keep a usable PATH after env_clear"
        );
        assert_eq!(
            env_map
                .get("RARA_SANDBOX_NETWORK_DISABLED")
                .map(String::as_str),
            Some("1")
        );
    }

    #[tokio::test]
    async fn pty_session_accepts_input_and_exposes_output() {
        let temp = tempdir().expect("tempdir");
        let sessions = Arc::new(PtySessionStore::new(temp.path().join("pty")).expect("pty store"));
        let write = PtyWriteTool {
            sessions: sessions.clone(),
        };
        let read = PtyReadTool {
            sessions: sessions.clone(),
        };

        let command = "read line; printf \"got:%s\\n\" \"$line\"".to_string();
        let started = sessions
            .start(
                command.clone(),
                WrappedCommand {
                    program: "/bin/sh".to_string(),
                    args: vec!["-c".to_string(), command],
                    cleanup_path: None,
                    sandboxed: false,
                    sandbox_backend: "direct".to_string(),
                    sandbox_home: None,
                    network_access: true,
                },
                temp.path().display().to_string(),
                &HashMap::new(),
                HashMap::new(),
                24,
                120,
            )
            .expect("start pty")
            .into_json(12_000)
            .await
            .expect("pty json");
        let session_id = started
            .get("session_id")
            .and_then(Value::as_str)
            .expect("session id")
            .to_string();
        assert_eq!(
            started.get("network_access").and_then(Value::as_bool),
            Some(true)
        );

        write
            .call(json!({
                "session_id": session_id,
                "input": "hello from pty\n",
            }))
            .await
            .expect("write pty");

        let mut last = Value::Null;
        for _ in 0..50 {
            last = read
                .call(json!({ "session_id": session_id, "tail_bytes": 4096 }))
                .await
                .expect("read pty");
            if last
                .get("output")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .contains("got:hello from pty")
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        let output = last
            .get("output")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            output.contains("got:hello from pty"),
            "last pty output did not contain expected marker: {last}"
        );
    }

    #[tokio::test]
    async fn pty_start_waits_briefly_for_quick_command_output() {
        let temp = tempdir().expect("tempdir");
        let sessions = Arc::new(PtySessionStore::new(temp.path().join("pty")).expect("pty store"));

        let command = "printf 'quick-done\\n'".to_string();
        let started = sessions
            .start(
                command.clone(),
                WrappedCommand {
                    program: "/bin/sh".to_string(),
                    args: vec!["-c".to_string(), command],
                    cleanup_path: None,
                    sandboxed: false,
                    sandbox_backend: "direct".to_string(),
                    sandbox_home: None,
                    network_access: true,
                },
                temp.path().display().to_string(),
                &HashMap::new(),
                HashMap::new(),
                24,
                120,
            )
            .expect("start pty");
        let inspected = sessions
            .wait_for_quick_completion(&started.id, PTY_START_QUICK_COMPLETION_TIMEOUT)
            .await
            .into_json(12_000)
            .await
            .expect("pty json");

        assert_eq!(
            inspected.get("status").and_then(Value::as_str),
            Some("completed")
        );
        assert!(
            inspected
                .get("output")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .contains("quick-done"),
            "quick command output should be available in pty_start result: {inspected}"
        );
    }

    #[tokio::test]
    async fn pty_start_keeps_long_running_session_running_after_brief_wait() {
        let temp = tempdir().expect("tempdir");
        let sessions = Arc::new(PtySessionStore::new(temp.path().join("pty")).expect("pty store"));

        let command = "sleep 2".to_string();
        let started = sessions
            .start(
                command.clone(),
                WrappedCommand {
                    program: "/bin/sh".to_string(),
                    args: vec!["-c".to_string(), command],
                    cleanup_path: None,
                    sandboxed: false,
                    sandbox_backend: "direct".to_string(),
                    sandbox_home: None,
                    network_access: true,
                },
                temp.path().display().to_string(),
                &HashMap::new(),
                HashMap::new(),
                24,
                120,
            )
            .expect("start pty");
        let inspected = sessions
            .wait_for_quick_completion(&started.id, Duration::from_millis(100))
            .await;

        assert_eq!(inspected.status, PtySessionStatus::Running);
        sessions.kill(&started.id).expect("cleanup pty");
    }

    #[tokio::test]
    async fn pty_kill_reports_killing_until_reader_observes_eof() {
        let temp = tempdir().expect("tempdir");
        let sessions = Arc::new(PtySessionStore::new(temp.path().join("pty")).expect("pty store"));

        let command = "sleep 30".to_string();
        let started = sessions
            .start(
                command.clone(),
                direct_shell_wrapped(&command),
                temp.path().display().to_string(),
                &HashMap::new(),
                HashMap::new(),
                24,
                120,
            )
            .expect("start pty");

        let killing = sessions.kill(&started.id).expect("kill pty");

        assert_eq!(killing.status, PtySessionStatus::Killing);
        assert_eq!(
            wait_for_session_status(
                &sessions,
                &started.id,
                PtySessionStatus::Killed,
                Duration::from_secs(3)
            )
            .await,
            Some(PtySessionStatus::Killed)
        );
    }

    #[tokio::test]
    async fn pty_kill_keeps_completed_session_completed() {
        let temp = tempdir().expect("tempdir");
        let sessions = Arc::new(PtySessionStore::new(temp.path().join("pty")).expect("pty store"));

        let command = "printf done".to_string();
        let started = sessions
            .start(
                command.clone(),
                direct_shell_wrapped(&command),
                temp.path().display().to_string(),
                &HashMap::new(),
                HashMap::new(),
                24,
                120,
            )
            .expect("start pty");
        let completed = sessions
            .wait_for_quick_completion(&started.id, PTY_START_QUICK_COMPLETION_TIMEOUT)
            .await;

        assert_eq!(completed.status, PtySessionStatus::Completed);
        let stopped = sessions.kill(&started.id).expect("kill completed pty");
        assert_eq!(stopped.status, PtySessionStatus::Completed);
    }

    #[test]
    fn pty_kill_error_restores_running_status() {
        let temp = tempdir().expect("tempdir");
        let sessions = PtySessionStore::new(temp.path().join("pty")).expect("pty store");
        let id = "pty-test".to_string();
        let status = Arc::new(Mutex::new(PtySessionStatus::Running));
        let record = PtySessionRecord {
            id: id.clone(),
            command: "failing".to_string(),
            output_path: temp.path().join("pty.log"),
            sandboxed: false,
            sandbox_backend: "direct".to_string(),
            network_access: true,
            writer: Arc::new(Mutex::new(Box::new(Vec::<u8>::new()))),
            child: Arc::new(Mutex::new(Box::new(FailingKillChild))),
            child_pid: None,
            status,
        };
        sessions
            .sessions
            .lock()
            .expect("pty session store lock")
            .insert(id.clone(), record);

        let err = match sessions.kill(&id) {
            Ok(_) => panic!("kill should fail"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("permission denied"));
        assert_eq!(
            sessions.get(&id).map(|snapshot| snapshot.status),
            Some(PtySessionStatus::Running)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pty_kill_terminates_background_children_in_process_group() {
        let temp = tempdir().expect("tempdir");
        let sessions = Arc::new(PtySessionStore::new(temp.path().join("pty")).expect("pty store"));

        let marker = "__rara_bg_pid:";
        let command = format!("sleep 1000 & bg=$!; echo {marker}$bg; wait");
        let started = sessions
            .start(
                command.clone(),
                direct_shell_wrapped(&command),
                temp.path().display().to_string(),
                &HashMap::new(),
                HashMap::new(),
                24,
                120,
            )
            .expect("start pty");
        let bg_pid = wait_for_marker_pid(&started.output_path, marker, Duration::from_secs(3))
            .await
            .expect("background pid marker");
        assert!(
            process_is_active(bg_pid),
            "expected background child pid {bg_pid} to exist before kill"
        );

        let killing = sessions.kill(&started.id).expect("kill pty");

        assert_eq!(killing.status, PtySessionStatus::Killing);
        let inactive = wait_for_process_inactive(bg_pid, Duration::from_secs(3)).await;
        if !inactive {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(bg_pid as i32),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        assert!(inactive, "background child pid {bg_pid} survived pty_kill");
        assert_eq!(
            wait_for_session_status(
                &sessions,
                &started.id,
                PtySessionStatus::Killed,
                Duration::from_secs(3)
            )
            .await,
            Some(PtySessionStatus::Killed)
        );
    }

    #[tokio::test]
    async fn pty_sessions_can_be_listed_statused_and_stopped() {
        let temp = tempdir().expect("tempdir");
        let sessions = Arc::new(PtySessionStore::new(temp.path().join("pty")).expect("pty store"));
        let list = PtyListTool {
            sessions: sessions.clone(),
        };
        let status = PtyStatusTool {
            sessions: sessions.clone(),
        };
        let stop = PtyStopTool {
            sessions: sessions.clone(),
        };

        let command = "sleep 30".to_string();
        let started = sessions
            .start(
                command.clone(),
                WrappedCommand {
                    program: "/bin/sh".to_string(),
                    args: vec!["-c".to_string(), command],
                    cleanup_path: None,
                    sandboxed: false,
                    sandbox_backend: "direct".to_string(),
                    sandbox_home: None,
                    network_access: true,
                },
                temp.path().display().to_string(),
                &HashMap::new(),
                HashMap::new(),
                24,
                120,
            )
            .expect("start pty")
            .into_json(12_000)
            .await
            .expect("pty json");
        let session_id = started
            .get("session_id")
            .and_then(Value::as_str)
            .expect("session id")
            .to_string();
        assert_eq!(
            started.get("network_access").and_then(Value::as_bool),
            Some(true)
        );

        let listed = list.call(json!({})).await.expect("list ptys");
        assert_eq!(
            listed
                .get("sessions")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            listed
                .pointer("/sessions/0/network_access")
                .and_then(Value::as_bool),
            Some(true)
        );

        let inspected = status
            .call(json!({ "session_id": session_id }))
            .await
            .expect("pty status");
        assert_eq!(
            inspected.get("status").and_then(Value::as_str),
            Some("running")
        );
        assert_eq!(
            inspected.get("network_access").and_then(Value::as_bool),
            Some(true)
        );

        let stopped = stop.call(json!({})).await.expect("stop all ptys");
        assert_eq!(
            stopped.pointer("/stopped/0/status").and_then(Value::as_str),
            Some("killing")
        );
        assert_eq!(
            stopped
                .pointer("/stopped/0/network_access")
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    fn direct_shell_wrapped(command: &str) -> WrappedCommand {
        WrappedCommand {
            program: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), command.to_string()],
            cleanup_path: None,
            sandboxed: false,
            sandbox_backend: "direct".to_string(),
            sandbox_home: None,
            network_access: true,
        }
    }

    #[derive(Debug)]
    struct FailingKillChild;

    impl portable_pty::ChildKiller for FailingKillChild {
        fn kill(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "permission denied",
            ))
        }

        fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
            Box::new(FailingKillChild)
        }
    }

    impl Child for FailingKillChild {
        fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
            Ok(None)
        }

        fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
            Ok(portable_pty::ExitStatus::with_exit_code(1))
        }

        fn process_id(&self) -> Option<u32> {
            None
        }
    }

    async fn wait_for_session_status(
        sessions: &PtySessionStore,
        id: &str,
        expected: PtySessionStatus,
        timeout: Duration,
    ) -> Option<PtySessionStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            let status = sessions.get(id).map(|snapshot| snapshot.status);
            if status == Some(expected) || Instant::now() >= deadline {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[cfg(unix)]
    async fn wait_for_marker_pid(path: &Path, marker: &str, timeout: Duration) -> Option<u32> {
        let deadline = Instant::now() + timeout;
        loop {
            let output = read_output_tail(path, 4096).await.ok()?;
            if let Some(pid) = parse_marker_pid(&output, marker) {
                return Some(pid);
            }
            if Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[cfg(unix)]
    fn parse_marker_pid(output: &str, marker: &str) -> Option<u32> {
        output.lines().find_map(|line| {
            let (_, tail) = line.split_once(marker)?;
            let pid = tail
                .chars()
                .take_while(|ch| ch.is_ascii_digit())
                .collect::<String>();
            pid.parse().ok()
        })
    }

    #[cfg(unix)]
    fn process_is_active(pid: u32) -> bool {
        process_state(pid).is_some_and(|state| !state.starts_with('Z'))
    }

    #[cfg(unix)]
    fn process_state(pid: u32) -> Option<String> {
        let output = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!state.is_empty()).then_some(state)
    }

    #[cfg(unix)]
    async fn wait_for_process_inactive(pid: u32, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if !process_is_active(pid) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

#[cfg(test)]
mod input_tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_structured_program_payload() {
        let input = PtyCommandInput::from_value(json!({
            "program": "cargo",
            "args": ["check", "--workspace"],
            "cwd": "/tmp",
            "allow_net": true,
            "rows": 30,
            "cols": 100,
        }))
        .expect("structured pty payload");

        assert_eq!(input.program.as_deref(), Some("cargo"));
        assert_eq!(
            input.args,
            vec!["check".to_string(), "--workspace".to_string()]
        );
        assert_eq!(input.cwd.as_deref(), Some("/tmp"));
        assert!(input.allow_net);
        assert_eq!(input.rows, 30);
        assert_eq!(input.cols, 100);
        assert_eq!(input.summary(), "cargo check --workspace");
    }

    #[test]
    fn parses_legacy_command_payload() {
        let input = PtyCommandInput::from_value(json!({
            "command": "echo hello",
        }))
        .expect("legacy pty payload");

        assert_eq!(input.command.as_deref(), Some("echo hello"));
        assert_eq!(input.summary(), "echo hello");
        assert_eq!(input.rows, 24);
        assert_eq!(input.cols, 120);
    }

    #[test]
    fn rejects_missing_command_and_program() {
        let err = PtyCommandInput::from_value(json!({
            "rows": 24,
            "cols": 120,
        }))
        .expect_err("no command or program");

        assert!(
            err.to_string().contains("either command or program"),
            "{err}"
        );
    }

    #[test]
    fn rejects_zero_rows() {
        let err = PtyCommandInput::from_value(json!({
            "command": "echo hi",
            "rows": 0,
        }))
        .expect_err("zero rows");

        assert!(err.to_string().contains("rows must be >= 1"), "{err}");
    }

    #[test]
    fn rejects_zero_cols() {
        let err = PtyCommandInput::from_value(json!({
            "command": "echo hi",
            "cols": 0,
        }))
        .expect_err("zero cols");

        assert!(err.to_string().contains("cols must be >= 1"), "{err}");
    }

    #[test]
    fn whitespace_command_falls_back_to_program() {
        let input = PtyCommandInput::from_value(json!({
            "command": "   ",
            "program": "cargo",
            "args": ["test"],
        }))
        .expect("whitespace command");

        assert_eq!(input.program.as_deref(), Some("cargo"));
        assert_eq!(input.summary(), "cargo test");
    }
}
