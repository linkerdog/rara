use std::collections::HashMap;
use std::env;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use rara_tools::tool::ToolError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::output::read_output_tail;
use crate::sandbox::SandboxManager;

pub struct PtyStartTool {
    pub sessions: Arc<crate::tools::pty::PtySessionStore>,
    pub sandbox: Arc<SandboxManager>,
    pub base_env: Arc<HashMap<String, String>>,
    pub sandbox_network_access: Arc<AtomicBool>,
}

pub struct PtyReadTool {
    pub sessions: Arc<crate::tools::pty::PtySessionStore>,
}

pub struct PtyListTool {
    pub sessions: Arc<crate::tools::pty::PtySessionStore>,
}

pub struct PtyStatusTool {
    pub sessions: Arc<crate::tools::pty::PtySessionStore>,
}

pub struct PtyWriteTool {
    pub sessions: Arc<crate::tools::pty::PtySessionStore>,
}

pub struct PtyKillTool {
    pub sessions: Arc<crate::tools::pty::PtySessionStore>,
}

pub struct PtyStopTool {
    pub sessions: Arc<crate::tools::pty::PtySessionStore>,
}

pub(crate) struct PtySessionRecord {
    pub(crate) id: String,
    pub(crate) command: String,
    pub(crate) output_path: PathBuf,
    pub(crate) sandboxed: bool,
    pub(crate) sandbox_backend: String,
    pub(crate) network_access: bool,
    pub(crate) writer: Arc<Mutex<Box<dyn Write + Send>>>,
    pub(crate) child: Arc<Mutex<Box<dyn portable_pty::Child + Send + Sync>>>,
    pub(crate) child_pid: Option<u32>,
    pub(crate) status: Arc<Mutex<PtySessionStatus>>,
    pub(crate) last_read: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PtySessionStatus {
    Running,
    Killing,
    Completed,
    Killed,
}

impl PtySessionStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Killing => "killing",
            Self::Completed => "completed",
            Self::Killed => "killed",
        }
    }
}

pub(crate) struct PtySessionSnapshot {
    pub(crate) id: String,
    pub(crate) command: String,
    pub(crate) output_path: PathBuf,
    pub(crate) sandboxed: bool,
    pub(crate) sandbox_backend: String,
    pub(crate) network_access: bool,
    pub(crate) status: PtySessionStatus,
}

impl PtySessionSnapshot {
    pub(crate) fn missing(id: &str) -> Self {
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

    pub(crate) fn metadata_json(self) -> Value {
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

    pub(crate) async fn into_json(self, tail_bytes: usize) -> Result<Value, ToolError> {
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

impl PtySessionRecord {
    pub(crate) fn snapshot(&self) -> PtySessionSnapshot {
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
