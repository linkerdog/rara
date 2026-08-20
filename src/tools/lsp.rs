use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use rara_tools::tool::{Tool, ToolError};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::lsp_manager::LspManager;

pub struct LspDiagnosticsTool {
    manager: Arc<LspManager>,
}

impl LspDiagnosticsTool {
    pub fn new(manager: Arc<LspManager>) -> Self {
        Self { manager }
    }
}

#[derive(Deserialize)]
struct LspDiagnosticsInput {
    file: PathBuf,
}

#[async_trait]
impl Tool for LspDiagnosticsTool {
    fn name(&self) -> &str {
        "lsp_diagnostics"
    }

    fn description(&self) -> &str {
        "Return current, cached, or pending Language Server Protocol diagnostics for a source file. Starts the matching local language server lazily and reports structured startup failures."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file": {
                    "type": "string",
                    "description": "Path to the source file to check, relative to the workspace root or absolute."
                }
            },
            "required": ["file"]
        })
    }

    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        let input: LspDiagnosticsInput = serde_json::from_value(input)
            .map_err(|err| ToolError::InvalidInput(err.to_string()))?;
        match self.manager.diagnostics_for(&input.file).await {
            Ok(result) => Ok(json!({
                "file": input.file.display().to_string(),
                "diagnostics": result.diagnostics,
                "freshness": result.freshness,
                "status": self.manager.status_snapshot(),
            })),
            Err(failure) => Ok(json!({
                "file": input.file.display().to_string(),
                "diagnostics": [],
                "error": failure.to_string(),
                "failure": failure,
                "status": self.manager.status_snapshot(),
            })),
        }
    }
}
