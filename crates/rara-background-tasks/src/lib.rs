//! Background task lifecycle management.
//!
//! Single responsibility: register, list, inspect status, stop, and read
//! output of background shell tasks. This crate owns the task store, record
//! types, state machine, and the three RARA tool implementations that expose
//! background task operations to the agent loop.

use std::collections::HashMap;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rara_tool_macros::tool_spec;
use rara_tools::tool::{Tool, ToolError, ToolOutputStream};
use serde::Serialize;
use serde_json::Value;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::oneshot;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Stream kind used by output file helpers.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BashStreamKind {
    Stdout,
    Stderr,
}

impl BashStreamKind {
    pub fn output_stream(self) -> ToolOutputStream {
        match self {
            Self::Stdout => ToolOutputStream::Stdout,
            Self::Stderr => ToolOutputStream::Stderr,
        }
    }
}

// ---------------------------------------------------------------------------
// Task lifecycle state machine
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskStatus {
    Running,
    Completed,
    Failed,
    Killed,
}

/// Higher-level classifier for background task lifecycle state.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskState {
    Working,
    Blocked,
    Done,
    Failed,
}

impl BackgroundTaskStatus {
    pub fn classify(self) -> BackgroundTaskState {
        match self {
            Self::Running | Self::Killed => BackgroundTaskState::Working,
            Self::Completed => BackgroundTaskState::Done,
            Self::Failed => BackgroundTaskState::Failed,
        }
    }
}

// ---------------------------------------------------------------------------
// Task record
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct BackgroundTaskRecord {
    pub id: String,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub output_path: PathBuf,
    pub status: BackgroundTaskStatus,
    pub exit_code: Option<i32>,
    pub sandboxed: bool,
    pub sandbox_backend: String,
    pub network_access: bool,
}

// ---------------------------------------------------------------------------
// Task store
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct BackgroundTaskStore {
    dir: PathBuf,
    tasks: Arc<Mutex<HashMap<String, BackgroundTaskRecord>>>,
    stop_signals: Arc<Mutex<HashMap<String, oneshot::Sender<()>>>>,
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

    pub fn start_record(
        &self,
        command: String,
        program: Option<String>,
        args: Vec<String>,
        cwd: Option<String>,
        sandboxed: bool,
        sandbox_backend: String,
        network_access: bool,
    ) -> Result<(BackgroundTaskRecord, oneshot::Receiver<()>), ToolError> {
        let id = format!("bash-{}", Uuid::new_v4());
        let output_path = self.dir.join(format!("{id}.log"));
        let record = BackgroundTaskRecord {
            id: id.clone(),
            command,
            program,
            args,
            cwd,
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
            .unwrap()
            .insert(id.clone(), record.clone());
        self.stop_signals.lock().unwrap().insert(id, stop_tx);
        Ok((record, stop_rx))
    }

    pub fn finish(&self, id: &str, status: BackgroundTaskStatus, exit_code: Option<i32>) {
        if let Some(record) = self.tasks.lock().unwrap().get_mut(id) {
            if !matches!(record.status, BackgroundTaskStatus::Killed) {
                record.status = status;
            }
            record.exit_code = exit_code;
        }
        self.stop_signals.lock().unwrap().remove(id);
    }

    pub fn get(&self, id: &str) -> Option<BackgroundTaskRecord> {
        self.tasks.lock().unwrap().get(id).cloned()
    }

    pub fn list(&self) -> Vec<BackgroundTaskRecord> {
        let mut records: Vec<_> = self.tasks.lock().unwrap().values().cloned().collect();
        records.sort_by(|left, right| left.id.cmp(&right.id));
        records
    }

    pub fn stop(&self, id: &str) -> Result<BackgroundTaskRecord, ToolError> {
        let mut tasks = self.tasks.lock().unwrap();
        let record = tasks
            .get_mut(id)
            .ok_or_else(|| ToolError::InvalidInput(format!("unknown task id: {id}")))?;
        if !matches!(record.status, BackgroundTaskStatus::Running) {
            return Ok(record.clone());
        }
        record.status = BackgroundTaskStatus::Killed;
        let stopped = record.clone();
        drop(tasks);

        if let Some(stop) = self.stop_signals.lock().unwrap().remove(id) {
            let _ = stop.send(());
        }
        Ok(stopped)
    }

    pub fn stop_all(&self) -> Vec<BackgroundTaskRecord> {
        let ids: Vec<_> = self
            .list()
            .into_iter()
            .filter(|record| matches!(record.status, BackgroundTaskStatus::Running))
            .map(|record| record.id)
            .collect();
        ids.into_iter()
            .filter_map(|id| self.stop(&id).ok())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Output-file helpers
// ---------------------------------------------------------------------------

pub async fn append_background_output(
    path: &Path,
    stream: BashStreamKind,
    chunk: &str,
) -> Result<(), ToolError> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    use tokio::io::AsyncWriteExt;
    match stream {
        BashStreamKind::Stdout => {
            file.write_all(chunk.as_bytes()).await?;
        }
        BashStreamKind::Stderr => {
            file.write_all(b"[stderr] ").await?;
            file.write_all(chunk.as_bytes()).await?;
        }
    }
    Ok(())
}

pub async fn read_output_tail(path: &Path, max_bytes: usize) -> Result<String, ToolError> {
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

// ---------------------------------------------------------------------------
// Tool implementations for the agent loop
// ---------------------------------------------------------------------------

pub struct BackgroundTaskListTool {
    pub background_tasks: Arc<BackgroundTaskStore>,
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
        Ok(serde_json::json!({
            "tasks": self.background_tasks.list(),
        }))
    }
}

pub struct BackgroundTaskStatusTool {
    pub background_tasks: Arc<BackgroundTaskStore>,
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

        Ok(serde_json::json!({
            "task_id": record.id,
            "command": record.command,
            "program": record.program,
            "args": record.args,
            "cwd": record.cwd,
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

pub struct BackgroundTaskStopTool {
    pub background_tasks: Arc<BackgroundTaskStore>,
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
            return Ok(serde_json::json!({ "stopped": [task] }));
        }
        Ok(serde_json::json!({ "stopped": self.background_tasks.stop_all() }))
    }
}

pub fn kill_child_process_group(child_id: Option<u32>) {
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

pub async fn read_stream_chunks<R>(
    reader: R,
    stream: BashStreamKind,
    tx: tokio::sync::mpsc::Sender<(BashStreamKind, String)>,
) -> Result<(), ToolError>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
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

pub async fn run_background_bash_task(
    child: &mut tokio::process::Child,
    record: &BackgroundTaskRecord,
    mut stop_rx: oneshot::Receiver<()>,
    process_group_id: Option<u32>,
    sandbox_warning: Option<&str>,
    cleanup_path: Option<&Path>,
) -> Result<Option<i32>, ToolError> {
    if let Some(parent) = record.output_path.parent() {
        fs::create_dir_all(parent).await?;
    }
    fs::write(&record.output_path, "").await?;
    if let Some(warning) = sandbox_warning {
        append_background_output(&record.output_path, BashStreamKind::Stderr, warning).await?;
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ToolError::ExecutionFailed("stdout pipe unavailable".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ToolError::ExecutionFailed("stderr pipe unavailable".into()))?;
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
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
    if let Some(path) = cleanup_path {
        let _ = fs::remove_file(path).await;
    }
    Ok(status.code())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_output_tail_returns_only_requested_suffix() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("task.log");
        tokio::fs::write(&path, "0123456789").await.unwrap();
        let tail = read_output_tail(&path, 3).await.unwrap();
        assert_eq!(tail, "789");
    }

    #[tokio::test]
    async fn read_output_tail_missing_file_is_empty() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("nope.log");
        let tail = read_output_tail(&path, 12_000).await.unwrap();
        assert!(tail.is_empty());
    }

    #[tokio::test]
    async fn start_stop_finish() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = BackgroundTaskStore::new(dir.path().to_path_buf()).unwrap();

        let (record, stop_rx) = store
            .start_record(
                "echo hi".into(),
                None,
                vec![],
                None,
                false,
                String::new(),
                false,
            )
            .unwrap();
        assert_eq!(record.status, BackgroundTaskStatus::Running);

        let stored = store.get(&record.id).unwrap();
        assert_eq!(stored.command, "echo hi");

        let list = store.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, record.id);

        drop(stop_rx);
        let stopped = store.stop(&record.id).unwrap();
        assert_eq!(stopped.status, BackgroundTaskStatus::Killed);
    }

    #[tokio::test]
    async fn stop_all_stops_running() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = BackgroundTaskStore::new(dir.path().to_path_buf()).unwrap();

        let (r1, rx1) = store
            .start_record(
                "cmd1".into(),
                None,
                vec![],
                None,
                false,
                String::new(),
                false,
            )
            .unwrap();
        let (r2, rx2) = store
            .start_record(
                "cmd2".into(),
                None,
                vec![],
                None,
                false,
                String::new(),
                false,
            )
            .unwrap();

        store.finish(&r1.id, BackgroundTaskStatus::Completed, Some(0));
        drop(rx1);

        drop(rx2);
        let stopped = store.stop_all();
        assert_eq!(stopped.len(), 1);
        assert_eq!(stopped[0].id, r2.id);
    }

    #[tokio::test]
    async fn append_and_read_output() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("out.log");

        append_background_output(&path, BashStreamKind::Stdout, "hello")
            .await
            .unwrap();
        append_background_output(&path, BashStreamKind::Stderr, "error")
            .await
            .unwrap();

        let tail = read_output_tail(&path, 100).await.unwrap();
        assert!(tail.contains("hello"));
        assert!(tail.contains("[stderr] error"));
    }
}
