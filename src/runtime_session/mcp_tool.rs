use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;
use rara_mcp_client::{CallToolResponse, McpToolDefinition, StdioConnection};
use rara_tools::tool::{Tool, ToolCallContext, ToolError, ToolProgressEvent};
use serde_json::Value;
use tokio::sync::Mutex;

pub(super) struct McpSourceTool {
    pub name: String,
    pub definition: McpToolDefinition,
    pub session_id: String,
    pub workspace: PathBuf,
    pub connection: Arc<Mutex<StdioConnection>>,
    pub active: Arc<AtomicBool>,
}

#[async_trait]
impl Tool for McpSourceTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        self.definition
            .description
            .as_deref()
            .filter(|description| !description.trim().is_empty())
            .unwrap_or(self.definition.name.as_ref())
    }

    fn input_schema(&self) -> Value {
        Value::Object((*self.definition.input_schema).clone())
    }

    async fn call(&self, _input: Value) -> Result<Value, ToolError> {
        Err(ToolError::InvalidInput(
            "controlled MCP tools require owned invocation context".into(),
        ))
    }

    async fn call_with_context_events(
        &self,
        input: Value,
        context: ToolCallContext,
        _report: &mut (dyn FnMut(ToolProgressEvent) + Send),
    ) -> Result<Value, ToolError> {
        if context.session_id() != Some(self.session_id.as_str())
            || context.workspace_root() != Some(self.workspace.as_path())
            || context.turn_id().is_none_or(str::is_empty)
            || context.call_id().is_none_or(str::is_empty)
        {
            return Err(ToolError::InvalidInput(
                "MCP invocation targets a foreign or incomplete context".into(),
            ));
        }
        if !self.active.load(Ordering::SeqCst) || context.is_cancelled() {
            return Err(ToolError::ExecutionFailed(
                "MCP source is retired or invocation is cancelled".into(),
            ));
        }
        let arguments = input
            .as_object()
            .ok_or_else(|| ToolError::InvalidInput("MCP arguments must be an object".into()))?
            .clone();
        let connection = self.connection.lock().await;
        if !self.active.load(Ordering::SeqCst) || context.is_cancelled() {
            return Err(ToolError::ExecutionFailed(
                "MCP source is retired or invocation is cancelled".into(),
            ));
        }
        let response = connection
            .call(&self.definition.name, arguments)
            .await
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
        let result = match response {
            CallToolResponse::Complete(result) => {
                if result.is_error.unwrap_or(false) {
                    return Err(ToolError::ExecutionFailed(
                        serde_json::to_string(&result).map_err(|_| {
                            ToolError::ExecutionFailed(
                                "MCP error response could not be represented".into(),
                            )
                        })?,
                    ));
                }
                serde_json::to_value(result)
            }
            CallToolResponse::InputRequired(result) => serde_json::to_value(result),
            CallToolResponse::Task(result) => serde_json::to_value(result),
            // New SDK response variants require an explicit forwarding decision.
            _ => {
                return Err(ToolError::ExecutionFailed(
                    "unsupported MCP response; execution outcome is uncertain".into(),
                ));
            }
        };
        result.map_err(|_| {
            ToolError::ExecutionFailed(
                "MCP response could not be represented; execution outcome is uncertain".into(),
            )
        })
    }
}
