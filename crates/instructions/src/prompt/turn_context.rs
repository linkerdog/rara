use std::path::Path;

use super::{PromptMode, PromptRuntimeConfig, PromptSource, PromptSourceKind};
use crate::workspace::WorkspaceMemory;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnPromptContext {
    pub environment: String,
    pub execution_mode: String,
    pub protocol_prompt_sources: Option<String>,
}

pub fn build_turn_prompt_context(
    workspace: &WorkspaceMemory,
    runtime: &PromptRuntimeConfig,
    mode: PromptMode,
) -> TurnPromptContext {
    let (cwd, branch) = workspace.get_env_info();
    TurnPromptContext {
        environment: render_environment_context(&cwd, &branch),
        execution_mode: render_mode_context(mode),
        protocol_prompt_sources: render_protocol_prompt_sources_section(
            runtime.protocol_prompt_sources.as_slice(),
        ),
    }
}

fn render_protocol_prompt_sources_section(sources: &[PromptSource]) -> Option<String> {
    let sections = sources
        .iter()
        .filter(|source| matches!(source.kind, PromptSourceKind::ProtocolPromptSource))
        .map(|source| {
            format!(
                "### {}\nSource: {}\n\n{}",
                source.label, source.display_path, source.content
            )
        })
        .collect::<Vec<_>>();
    if sections.is_empty() {
        None
    } else {
        Some(format!(
            "## Protocol Prompt Sources\n\n{}",
            sections.join("\n\n")
        ))
    }
}

pub(super) fn render_environment_context(cwd: &str, branch: &str) -> String {
    let shell = std::env::var("SHELL")
        .ok()
        .and_then(|value| {
            Path::new(&value)
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    format!(
        "<environment_context>\n  \
         <cwd>{}</cwd>\n  \
         <shell>{}</shell>\n  \
         <git_branch>{}</git_branch>\n  \
         Note: This snapshot was observed before the current user turn.\n\
         </environment_context>",
        escape_xml_text(cwd),
        escape_xml_text(&shell),
        escape_xml_text(branch),
    )
}

fn escape_xml_text(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            ch => escaped.push(ch),
        }
    }
    escaped
}

fn render_mode_context(mode: PromptMode) -> String {
    match mode {
        PromptMode::Execute => super::execute_mode_prompt(),
        PromptMode::Plan => super::plan_mode_prompt(),
        PromptMode::Review => super::review_mode_prompt(),
    }
}
