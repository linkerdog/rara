use std::fs;
use std::path::Path;

use async_trait::async_trait;
use glob::glob;
use rara_tool_macros::tool_spec;
use regex::Regex;
use serde_json::{Value, json};

use crate::tool::{Tool, ToolError};

pub struct GlobTool;
#[tool_spec(
    name = "glob",
    description = "Find files matching glob pattern",
    input_schema = {
        "type": "object",
        "properties": { "pattern": { "type": "string" } },
        "required": ["pattern"]
    }
)]
#[async_trait]
impl Tool for GlobTool {
    async fn call(&self, i: Value) -> Result<Value, ToolError> {
        let p = i["pattern"]
            .as_str()
            .ok_or(ToolError::InvalidInput("pattern".into()))?;
        let mut matches = Vec::new();
        for entry in glob(p).map_err(|e| ToolError::InvalidInput(e.to_string()))? {
            if let Ok(path) = entry {
                matches.push(path.display().to_string());
            }
        }
        Ok(json!({ "matches": matches }))
    }
}

pub struct GrepTool;
#[tool_spec(
    name = "grep",
    description = "Regex search in files",
    input_schema = {
        "type": "object",
        "properties": {
            "pattern": { "type": "string" },
            "path": { "type": "string", "default": "." },
            "include_ignored": { "type": "boolean", "default": false }
        },
        "required": ["pattern"]
    }
)]
#[async_trait]
impl Tool for GrepTool {
    async fn call(&self, i: Value) -> Result<Value, ToolError> {
        let p = i["pattern"]
            .as_str()
            .ok_or(ToolError::InvalidInput("pattern".into()))?;
        let search_path = i["path"].as_str().unwrap_or(".");
        let include_ignored = i
            .get("include_ignored")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let re = Regex::new(p).map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let mut results = Vec::new();
        for entry in walkdir::WalkDir::new(search_path)
            .into_iter()
            .filter_entry(|entry| include_ignored || !is_ignored_path(entry.path()))
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                if let Ok(c) = fs::read_to_string(entry.path()) {
                    for (line_idx, line) in c.lines().enumerate() {
                        if re.is_match(line) {
                            results.push(json!({ "file": entry.path().display().to_string(), "line": line_idx + 1, "content": line.trim() }));
                        }
                    }
                }
            }
            if results.len() > 100 {
                break;
            }
        }
        Ok(json!({ "results": results }))
    }
}

fn is_ignored_path(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        matches!(
            name.as_ref(),
            ".git"
                | "target"
                | "node_modules"
                | "dist"
                | "build"
                | ".next"
                | ".cache"
                | "__pycache__"
                | ".venv"
                | "venv"
        )
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::GrepTool;
    use crate::tool::Tool;

    #[tokio::test]
    async fn grep_skips_build_artifacts_by_default() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let root = tempdir.path();
        std::fs::create_dir_all(root.join("src")).expect("mkdir src");
        std::fs::create_dir_all(root.join("target/debug")).expect("mkdir target");
        std::fs::write(root.join("src/main.rs"), "agent loop\n").expect("write source");
        std::fs::write(root.join("target/debug/app"), "agent loop\n").expect("write artifact");

        let tool = GrepTool;
        let result = tool
            .call(json!({
                "pattern": "agent loop",
                "path": root.display().to_string()
            }))
            .await
            .expect("grep succeeds");

        let results = result["results"].as_array().expect("results array");
        assert_eq!(results.len(), 1);
        let file = results[0]["file"].as_str().expect("file path");
        assert!(file.contains("src/main.rs"));
    }
}
