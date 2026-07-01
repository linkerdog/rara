use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;
use rara_tool_macros::tool_spec;
use rara_tools::tool::{Tool, ToolError};
use serde_json::{Value, json};
use tempfile::tempdir;

use crate::agent::Message;
use crate::llm::LlmBackend;
use crate::llm::LlmResponse;
use crate::session::SessionManager;
use crate::workspace::WorkspaceMemory;

pub(super) struct StubTool;
pub(super) struct StubBashTool;

#[tool_spec(
    name = "stub_tool",
    description = "Return a simple structured result",
    input_schema = { "type": "object" }
)]
#[async_trait]
impl Tool for StubTool {
    async fn call(&self, _input: Value) -> Result<Value, ToolError> {
        Ok(json!({ "status": "ok", "value": 42 }))
    }
}

#[tool_spec(
    name = "bash",
    description = "Return a simple bash result",
    input_schema = { "type": "object" }
)]
#[async_trait]
impl Tool for StubBashTool {
    async fn call(&self, _input: Value) -> Result<Value, ToolError> {
        Ok(json!({ "stdout": "ok\n", "stderr": "", "exit_code": 0 }))
    }
}

pub(super) struct SequencedBackend {
    responses: Mutex<Vec<LlmResponse>>,
    observed_messages: Mutex<Vec<Vec<Message>>>,
    observed_tools: Mutex<Vec<Vec<String>>>,
    model_label: Mutex<Option<String>>,
}

impl SequencedBackend {
    pub(super) fn new(responses: Vec<LlmResponse>) -> Self {
        Self {
            responses: Mutex::new(responses),
            observed_messages: Mutex::new(Vec::new()),
            observed_tools: Mutex::new(Vec::new()),
            model_label: Mutex::new(None),
        }
    }

    pub(super) fn with_model_label(self, model_label: impl Into<String>) -> Self {
        *self.model_label.lock().expect("lock") = Some(model_label.into());
        self
    }

    pub(super) fn observed_tools(&self) -> Vec<Vec<String>> {
        self.observed_tools.lock().expect("lock").clone()
    }

    pub(super) fn observed_messages(&self) -> Vec<Vec<Message>> {
        self.observed_messages.lock().expect("lock").clone()
    }
}

pub(super) fn test_runtime_storage() -> (
    tempfile::TempDir,
    Arc<SessionManager>,
    Arc<WorkspaceMemory>,
    std::path::PathBuf,
) {
    let temp = tempdir().expect("tempdir");
    let root = temp.path().to_path_buf();
    let rara_dir = root.join(".rara");
    std::fs::create_dir_all(rara_dir.join("rollouts")).expect("rollouts");
    std::fs::create_dir_all(rara_dir.join("sessions")).expect("sessions");
    std::fs::create_dir_all(rara_dir.join("tool-results")).expect("tool results");
    let session_manager = Arc::new(SessionManager {
        storage_dir: rara_dir.join("rollouts"),
        legacy_storage_dir: rara_dir.join("sessions"),
    });
    let workspace = Arc::new(WorkspaceMemory::from_paths(root, rara_dir.clone()));
    (temp, session_manager, workspace, rara_dir)
}

#[async_trait]
impl LlmBackend for SequencedBackend {
    fn model_label(&self) -> Option<String> {
        self.model_label.lock().expect("lock").clone()
    }

    async fn ask(&self, messages: &[Message], tools: &[Value]) -> Result<LlmResponse> {
        self.observed_messages
            .lock()
            .expect("lock")
            .push(messages.to_vec());
        self.observed_tools.lock().expect("lock").push(
            tools
                .iter()
                .filter_map(|tool| tool.get("name").and_then(Value::as_str).map(str::to_string))
                .collect(),
        );
        let mut responses = self.responses.lock().expect("lock");
        assert!(
            !responses.is_empty(),
            "test backend ran out of scripted responses"
        );
        Ok(responses.remove(0))
    }

    async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
        Ok(vec![0.0; 8])
    }

    async fn summarize(&self, _messages: &[Message], _instruction: &str) -> Result<String> {
        Ok("summary".to_string())
    }
}
