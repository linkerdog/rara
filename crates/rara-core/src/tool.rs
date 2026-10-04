use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("Invalid input: {0}")]
    InvalidInput(String),
    #[error("Execution failed: {0}")]
    ExecutionFailed(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolOutputStream {
    Stdout,
    Stderr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolProgressEvent {
    Output {
        stream: ToolOutputStream,
        chunk: String,
    },
}

/// Tool progress observer using the host target's threading convention.
/// In `async_trait` methods, use the generated `'async_trait` alias lifetime.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub type ToolProgressCallback<'a> = dyn FnMut(ToolProgressEvent) + Send + 'a;
/// Browser progress observers may retain JavaScript-owned local state.
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub type ToolProgressCallback<'a> = dyn FnMut(ToolProgressEvent) + 'a;

#[derive(Clone, Debug, Default)]
pub struct ToolCallContext {
    cancellation: Option<Arc<AtomicBool>>,
    session_id: Option<String>,
    turn_id: Option<String>,
    call_id: Option<String>,
    workspace_root: Option<PathBuf>,
    inference: Option<rara_observability::InferenceAgentContext>,
}

impl ToolCallContext {
    pub fn with_inference(mut self, context: rara_observability::InferenceAgentContext) -> Self {
        self.inference = Some(context);
        self
    }

    pub fn inference(&self) -> Option<&rara_observability::InferenceAgentContext> {
        self.inference.as_ref()
    }

    pub fn with_cancellation(mut self, cancellation: Arc<AtomicBool>) -> Self {
        self.cancellation = Some(cancellation);
        self
    }

    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn with_turn_id(mut self, turn_id: impl Into<String>) -> Self {
        self.turn_id = Some(turn_id.into());
        self
    }

    pub fn with_call_id(mut self, call_id: impl Into<String>) -> Self {
        self.call_id = Some(call_id.into());
        self
    }

    pub fn with_workspace_root(mut self, workspace_root: impl Into<PathBuf>) -> Self {
        self.workspace_root = Some(workspace_root.into());
        self
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub fn turn_id(&self) -> Option<&str> {
        self.turn_id.as_deref()
    }

    pub fn call_id(&self) -> Option<&str> {
        self.call_id.as_deref()
    }

    pub fn workspace_root(&self) -> Option<&Path> {
        self.workspace_root.as_deref()
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancellation
            .as_ref()
            .is_some_and(|cancellation| cancellation.load(Ordering::SeqCst))
    }
}

/// Executable tool contract shared by application and embedding hosts.
///
/// Implementors keep their schema and executable behavior consistent. Trusted
/// identity belongs in `ToolCallContext`, separately from model arguments.
/// Cancellation is cooperative: context-aware tools must observe the token.
#[cfg_attr(all(target_arch = "wasm32", target_os = "unknown"), async_trait(?Send))]
#[cfg_attr(not(all(target_arch = "wasm32", target_os = "unknown")), async_trait)]
pub trait Tool: crate::PlatformSend + crate::PlatformSync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> Value;
    async fn call(&self, input: Value) -> Result<Value, ToolError>;
    async fn call_with_events(
        &self,
        input: Value,
        _report: &mut ToolProgressCallback<'async_trait>,
    ) -> Result<Value, ToolError> {
        self.call(input).await
    }

    async fn call_with_context_events(
        &self,
        input: Value,
        _context: ToolCallContext,
        report: &mut ToolProgressCallback<'async_trait>,
    ) -> Result<Value, ToolError> {
        self.call_with_events(input, report).await
    }
}

pub struct ToolManager {
    tools: BTreeMap<String, Box<dyn Tool>>,
}

impl ToolManager {
    pub fn new() -> Self {
        Self {
            tools: BTreeMap::new(),
        }
    }
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }
    pub fn get_tool(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|b| b.as_ref())
    }
    pub fn get_schemas(&self) -> Vec<Value> {
        self.get_schemas_filtered(|_| true)
    }

    pub fn retain(&mut self, mut predicate: impl FnMut(&str) -> bool) {
        self.tools.retain(|name, _| predicate(name));
    }

    pub fn get_schemas_filtered<F>(&self, mut include: F) -> Vec<Value>
    where
        F: FnMut(&str) -> bool,
    {
        self.tools
            .iter()
            .filter_map(|(name, tool)| {
                if !include(name.as_str()) {
                    return None;
                }
                Some(serde_json::json!({
                    "name": tool.name(),
                    "description": tool.description(),
                    "input_schema": tool.input_schema(),
                }))
            })
            .collect()
    }
}

impl Default for ToolManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
