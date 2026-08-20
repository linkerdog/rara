use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use serde_json::{Value, json};

use super::render::{
    INLINE_CHAR_BUDGET, compact_apply_patch, compact_bash, compact_generic, compact_glob,
    compact_grep, compact_list_files, compact_lsp_diagnostics, compact_read_file, compact_replace,
    compact_replace_lines, compact_subagent_result, compact_web_fetch, compact_web_search,
    compact_write_file, render_persisted_compact_result, summarize_tool_result,
};

pub struct ToolResultStore {
    base_dir: PathBuf,
}

impl ToolResultStore {
    pub fn new(base_dir: impl Into<PathBuf>) -> Result<Self> {
        let base_dir = base_dir.into();
        if !base_dir.exists() {
            fs::create_dir_all(&base_dir)?;
        }
        Ok(Self { base_dir })
    }

    pub fn compact_result(
        &self,
        tool_name: &str,
        tool_use_id: &str,
        input: &Value,
        result: &Value,
    ) -> Result<String> {
        let summary = summarize_tool_result(tool_name, input, result);
        let inline = match tool_name {
            "bash" => compact_bash(result),
            "apply_patch" => compact_apply_patch(result),
            "write_file" => compact_write_file(result),
            "replace" => compact_replace(input, result),
            "replace_lines" => compact_replace_lines(input, result),
            "spawn_agent" | "explore_agent" | "plan_agent" => {
                compact_subagent_result(tool_name, result)
            }
            "list_files" => compact_list_files(input, result),
            "read_file" => compact_read_file(input, result),
            "glob" => compact_glob(result),
            "grep" => compact_grep(result),
            "web_fetch" => compact_web_fetch(input, result),
            "web_search" => compact_web_search(input, result),
            "lsp_diagnostics" => compact_lsp_diagnostics(result),
            _ => compact_generic(&summary, result),
        };
        let full_rendered =
            serde_json::to_string_pretty(result).unwrap_or_else(|_| result.to_string());
        let should_persist = full_rendered.chars().count() > INLINE_CHAR_BUDGET
            || inline.chars().count() > INLINE_CHAR_BUDGET;

        if !should_persist {
            return Ok(inline);
        }

        let stored_path = self.persist_full_result(tool_use_id, tool_name, input, result)?;
        Ok(render_persisted_compact_result(&inline, &stored_path))
    }

    fn persist_full_result(
        &self,
        tool_use_id: &str,
        tool_name: &str,
        input: &Value,
        result: &Value,
    ) -> Result<PathBuf> {
        let path = self.base_dir.join(format!("{tool_use_id}.json"));
        let summary = summarize_tool_result(tool_name, input, result);
        let payload = json!({
            "tool_name": tool_name,
            "summary": summary,
            "input": input,
            "result": result,
        });
        fs::write(&path, serde_json::to_string_pretty(&payload)?)?;
        Ok(path)
    }
}

pub fn default_tool_result_store_dir() -> Result<PathBuf> {
    let root = std::env::current_dir()?;
    Ok(rara_config::workspace_data_dir_for(&root)?.join("tool-results"))
}
