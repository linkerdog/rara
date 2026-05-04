use std::collections::HashMap;
use std::env;
use std::fs::OpenOptions;
use std::io::{Read, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use rara_tool_macros::tool_spec;
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
    pub sandbox_network_access: bool,
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
    status: Arc<Mutex<PtySessionStatus>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PtySessionStatus {
    Running,
    Completed,
    Killed,
}

impl PtySessionStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
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
            if matches!(*status, PtySessionStatus::Running) {
                *status = PtySessionStatus::Completed;
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
        let (child, status, mut snapshot) = {
            let sessions = self.sessions.lock().expect("pty session store lock");
            let record = sessions
                .get(id)
                .ok_or_else(|| ToolError::InvalidInput(format!("unknown pty session id: {id}")))?;
            (
                record.child.clone(),
                record.status.clone(),
                record.snapshot(),
            )
        };
        child
            .lock()
            .expect("pty child lock")
            .kill()
            .map_err(|err| ToolError::ExecutionFailed(format!("kill pty session: {err}")))?;
        *status.lock().expect("pty status lock") = PtySessionStatus::Killed;
        snapshot.status = PtySessionStatus::Killed;
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
        "required": ["command"]
    }
)]
#[async_trait]
impl Tool for PtyStartTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        let command = input["command"]
            .as_str()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| ToolError::InvalidInput("command".into()))?
            .to_string();
        let cwd = input
            .get("cwd")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                env::current_dir()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|_| ".".to_string())
            });
        let env = parse_env(input.get("env"))?;
        let allow_net = input
            .get("allow_net")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let allow_net = self.sandbox_network_access || allow_net;
        let wrapped = self
            .sandbox
            .wrap_pty_shell_command(&command, &cwd, allow_net)
            .map_err(|err| {
                ToolError::ExecutionFailed(format!("{} {}", err, sandbox_failure_hint()))
            })?;
        let rows = parse_pty_dimension(input.get("rows"), 24, "rows")?;
        let cols = parse_pty_dimension(input.get("cols"), 120, "cols")?;
        let started =
            self.sessions
                .start(command, wrapped, cwd, &self.base_env, env, rows, cols)?;
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

fn parse_env(value: Option<&Value>) -> Result<HashMap<String, String>, ToolError> {
    let Some(value) = value else {
        return Ok(HashMap::new());
    };
    serde_json::from_value(value.clone())
        .map_err(|err| ToolError::InvalidInput(format!("env: {err}")))
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
            sandbox_network_access: false,
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
            Some("killed")
        );
        assert_eq!(
            stopped
                .pointer("/stopped/0/network_access")
                .and_then(Value::as_bool),
            Some(true)
        );
    }
}
