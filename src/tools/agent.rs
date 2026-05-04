use std::sync::Arc;

use async_trait::async_trait;
use rara_tool_macros::tool_spec;
use serde_json::{Value, json};

use crate::agent::{Agent, AgentExecutionMode, Message, PendingUserInput, PlanStep};
use crate::llm::LlmBackend;
use crate::prompt::PromptRuntimeConfig;
use crate::session::SessionManager;
use crate::tool::{Tool, ToolError, ToolManager};
use crate::tools::file::{ListFilesTool, ReadFileTool};
use crate::tools::search::{GlobTool, GrepTool};
use crate::vectordb::VectorDB;
use crate::workspace::WorkspaceMemory;

#[derive(Clone, Copy)]
enum SubAgentKind {
    General,
    Explore,
    Plan,
}

macro_rules! strict_read_only_subagent_prompt {
    () => {
        concat!(
            "## Strict Read-Only Contract\n",
            "- This is a STRICT READ-ONLY sub-agent task.\n",
            "- You are prohibited from creating, modifying, deleting, moving, or copying files.\n",
            "- Do not create temporary files anywhere, including /tmp.\n",
            "- Do not run shell commands, scripts, redirection, heredocs, or any workaround that changes filesystem, process, network, git, or repository state.\n",
            "- Bash, PTY, editing, patching, and agent-spawning tools are intentionally unavailable.\n",
            // Keep this prompt list synchronized with build_read_only_tool_manager().
            "- Use only the read-only repository inspection tools available to you: read_file, list_files, glob, grep.\n",
            "- If the assigned instruction requires mutation, report the limitation and provide the evidence-backed findings or plan instead of attempting a workaround."
        )
    };
}

impl SubAgentKind {
    fn result_status(self) -> &'static str {
        match self {
            SubAgentKind::General => "done",
            SubAgentKind::Explore => "explored",
            SubAgentKind::Plan => "planned",
        }
    }

    fn append_prompt(self) -> &'static str {
        match self {
            SubAgentKind::General => {
                concat!(
                    "## Sub-Agent Role\n",
                    "- You are a no-tool reasoning sub-agent.\n",
                    "- Treat the assigned instruction as the complete task contract.\n",
                    "- Honor every constraint in the assigned instruction, including workspace, branch, network, and output limits.\n",
                    "- Stay inside the current workspace unless the assigned instruction explicitly allows another path.\n",
                    "- You do not have repository, shell, editing, patching, or browser tools in this role.\n",
                    "- If the assigned instruction requires inspection or mutation, report the limitation and answer only from the provided instruction/context.\n",
                    "- Do not delegate to another agent or spawn sub-agents; complete the assigned work directly."
                )
            }
            SubAgentKind::Explore => {
                concat!(
                    "## Sub-Agent Role\n",
                    "- You are a read-only exploration sub-agent.\n",
                    "- Treat the assigned instruction as the complete task contract.\n",
                    "- Honor every constraint in the assigned instruction, including workspace, branch, network, and output limits.\n",
                    "- Stay inside the current workspace unless the assigned instruction explicitly allows another path.\n",
                    "\n",
                    strict_read_only_subagent_prompt!(),
                    "\n",
                    "- Inspect the repository and summarize concrete findings.\n",
                    "- Do not propose edits you cannot justify from inspected code.\n",
                    "- Do not narrate each next tool call; call the tool directly.\n",
                    "- Do not delegate to another agent or spawn sub-agents; inspect and answer directly.\n",
                    "- End with a concise findings summary."
                )
            }
            SubAgentKind::Plan => {
                concat!(
                    "## Sub-Agent Role\n",
                    "- You are a read-only planning sub-agent.\n",
                    "- Treat the assigned instruction as the complete task contract.\n",
                    "- Honor every constraint in the assigned instruction, including workspace, branch, network, and output limits.\n",
                    "- Stay inside the current workspace unless the assigned instruction explicitly allows another path.\n",
                    "\n",
                    strict_read_only_subagent_prompt!(),
                    "\n",
                    "- Inspect the repository and refine an implementation approach.\n",
                    "- Keep plans shallow and grouped by behavior.\n",
                    "- Use <proposed_plan> only when the plan is decision-complete.\n",
                    "- If the plan is not ready, summarize what additional inspection is still needed and end with <continue_inspection/>.\n",
                    "- Do not stop with narration alone.\n",
                    "- Do not delegate to another agent or spawn sub-agents; inspect and answer directly.\n",
                    "- End with exactly one of: <proposed_plan>, <request_user_input>, or <continue_inspection/>."
                )
            }
        }
    }

    fn execution_mode(self) -> AgentExecutionMode {
        match self {
            SubAgentKind::Plan => AgentExecutionMode::Plan,
            SubAgentKind::General | SubAgentKind::Explore => AgentExecutionMode::Execute,
        }
    }

    fn read_only(self) -> bool {
        !matches!(self, SubAgentKind::General)
    }
}

pub struct AgentTool {
    pub backend: Arc<dyn LlmBackend>,
    pub vdb: Arc<VectorDB>,
    pub session_manager: Arc<SessionManager>,
    pub workspace: Arc<WorkspaceMemory>,
    pub prompt_config: PromptRuntimeConfig,
}

#[tool_spec(
    name = "spawn_agent",
    description = "Spawn a no-tool reasoning sub-agent. It cannot inspect files, run shell commands, edit files, or spawn other agents; use explore_agent or plan_agent for read-only repository inspection.",
    input_schema = {
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "instruction": { "type": "string" }
        },
        "required": ["name", "instruction"]
    }
)]
#[async_trait]
impl Tool for AgentTool {
    async fn call(&self, i: Value) -> Result<Value, ToolError> {
        let name = i["name"].as_str().unwrap_or("worker");
        let instruction = i["instruction"]
            .as_str()
            .ok_or(ToolError::InvalidInput("instruction".into()))?;
        let result = run_sub_agent(
            SubAgentKind::General,
            instruction,
            self.backend.clone(),
            self.vdb.clone(),
            self.session_manager.clone(),
            self.workspace.clone(),
            self.prompt_config.clone(),
        )
        .await?;
        Ok(json!({
            "name": name,
            "status": result.status,
            "summary": result.summary,
            "request_user_input": result
                .request_user_input
                .as_ref()
                .map(serialize_pending_user_input),
        }))
    }
}

pub struct ExploreAgentTool {
    pub backend: Arc<dyn LlmBackend>,
    pub vdb: Arc<VectorDB>,
    pub session_manager: Arc<SessionManager>,
    pub workspace: Arc<WorkspaceMemory>,
    pub prompt_config: PromptRuntimeConfig,
}

#[tool_spec(
    name = "explore_agent",
    description = "Spawn a read-only exploration sub-agent for bounded independent sidecar repository inspection. The instruction must be self-contained and include all user constraints.",
    input_schema = {
        "type": "object",
        "properties": {
            "instruction": { "type": "string" }
        },
        "required": ["instruction"]
    }
)]
#[async_trait]
impl Tool for ExploreAgentTool {
    async fn call(&self, i: Value) -> Result<Value, ToolError> {
        let instruction = i["instruction"]
            .as_str()
            .ok_or(ToolError::InvalidInput("instruction".into()))?;
        let result = run_sub_agent(
            SubAgentKind::Explore,
            instruction,
            self.backend.clone(),
            self.vdb.clone(),
            self.session_manager.clone(),
            self.workspace.clone(),
            self.prompt_config.clone(),
        )
        .await?;
        Ok(json!({
            "status": result.status,
            "summary": result.summary,
            "request_user_input": result
                .request_user_input
                .as_ref()
                .map(serialize_pending_user_input),
        }))
    }
}

pub struct PlanAgentTool {
    pub backend: Arc<dyn LlmBackend>,
    pub vdb: Arc<VectorDB>,
    pub session_manager: Arc<SessionManager>,
    pub workspace: Arc<WorkspaceMemory>,
    pub prompt_config: PromptRuntimeConfig,
}

#[tool_spec(
    name = "plan_agent",
    description = "Spawn a read-only planning sub-agent for bounded independent sidecar plan refinement. The instruction must be self-contained and include all user constraints.",
    input_schema = {
        "type": "object",
        "properties": {
            "instruction": { "type": "string" }
        },
        "required": ["instruction"]
    }
)]
#[async_trait]
impl Tool for PlanAgentTool {
    async fn call(&self, i: Value) -> Result<Value, ToolError> {
        let instruction = i["instruction"]
            .as_str()
            .ok_or(ToolError::InvalidInput("instruction".into()))?;
        let result = run_sub_agent(
            SubAgentKind::Plan,
            instruction,
            self.backend.clone(),
            self.vdb.clone(),
            self.session_manager.clone(),
            self.workspace.clone(),
            self.prompt_config.clone(),
        )
        .await?;
        Ok(json!({
            "status": result.status,
            "summary": result.summary,
            "plan": result
                .plan
                .as_ref()
                .map(|steps| serialize_plan_steps(steps)),
            "plan_explanation": result.plan_explanation,
            "request_user_input": result
                .request_user_input
                .as_ref()
                .map(serialize_pending_user_input),
        }))
    }
}

pub struct TeamCreateTool {
    pub backend: Arc<dyn LlmBackend>,
    pub vdb: Arc<VectorDB>,
    pub session_manager: Arc<SessionManager>,
    pub workspace: Arc<WorkspaceMemory>,
}

#[tool_spec(
    name = "team_create",
    description = "Launch parallel sub-agents",
    input_schema = {
        "type": "object",
        "properties": { "tasks": { "type": "array" } },
        "required": ["tasks"]
    }
)]
#[async_trait]
impl Tool for TeamCreateTool {
    async fn call(&self, i: Value) -> Result<Value, ToolError> {
        let tasks = i["tasks"]
            .as_array()
            .ok_or(ToolError::InvalidInput("tasks".into()))?;
        let mut results = Vec::new();
        for task in tasks {
            let name = task["name"].as_str().unwrap_or("worker");
            results.push(json!({ "name": name, "status": "mocked_done" }));
        }
        Ok(json!({ "team_results": results }))
    }
}

struct SubAgentResult {
    status: &'static str,
    summary: String,
    plan: Option<Vec<PlanStep>>,
    plan_explanation: Option<String>,
    request_user_input: Option<PendingUserInput>,
}

async fn run_sub_agent(
    kind: SubAgentKind,
    instruction: &str,
    backend: Arc<dyn LlmBackend>,
    vdb: Arc<VectorDB>,
    session_manager: Arc<SessionManager>,
    workspace: Arc<WorkspaceMemory>,
    prompt_config: PromptRuntimeConfig,
) -> Result<SubAgentResult, ToolError> {
    let tool_manager = build_subagent_tool_manager(kind);
    let mut sub = Agent::new(tool_manager, backend, vdb, session_manager, workspace);
    sub.set_execution_mode(kind.execution_mode());
    sub.set_prompt_config(append_subagent_prompt(prompt_config, kind.append_prompt()));
    sub.query_with_mode(
        instruction.to_string(),
        crate::agent::AgentOutputMode::Silent,
    )
    .await
    .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

    Ok(SubAgentResult {
        status: kind.result_status(),
        summary: latest_assistant_text(&sub).unwrap_or_else(|| "Sub-agent finished.".to_string()),
        plan: (!sub.current_plan.is_empty()).then_some(sub.current_plan.clone()),
        plan_explanation: sub.plan_explanation.clone(),
        request_user_input: sub.pending_user_input.clone(),
    })
}

fn build_read_only_tool_manager() -> ToolManager {
    // Keep this registration set synchronized with strict_read_only_subagent_prompt!().
    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(ReadFileTool::default()));
    tool_manager.register(Box::new(ListFilesTool));
    tool_manager.register(Box::new(GlobTool));
    tool_manager.register(Box::new(GrepTool));
    tool_manager
}

fn build_subagent_tool_manager(kind: SubAgentKind) -> ToolManager {
    if kind.read_only() {
        build_read_only_tool_manager()
    } else {
        ToolManager::new()
    }
}

fn append_subagent_prompt(
    mut prompt_config: PromptRuntimeConfig,
    appended_instructions: &str,
) -> PromptRuntimeConfig {
    if appended_instructions.trim().is_empty() {
        return prompt_config;
    }
    prompt_config.append_system_prompt = Some(match prompt_config.append_system_prompt.take() {
        Some(existing) if !existing.trim().is_empty() => {
            format!("{existing}\n\n{appended_instructions}")
        }
        _ => appended_instructions.to_string(),
    });
    prompt_config
}

fn latest_assistant_text_from_history(history: &[Message]) -> Option<String> {
    history.iter().rev().find_map(|message| {
        if message.role != "assistant" {
            return None;
        }
        if let Some(text) = message.content.as_str() {
            let trimmed = text.trim();
            return (!trimmed.is_empty()).then(|| trimmed.to_string());
        }
        message.content.as_array().and_then(|blocks| {
            let text = blocks
                .iter()
                .filter_map(|block| {
                    block
                        .get("type")
                        .and_then(Value::as_str)
                        .zip(block.get("text").and_then(Value::as_str))
                })
                .filter_map(|(kind, text)| (kind == "text").then_some(text))
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_string();
            (!text.is_empty()).then_some(text)
        })
    })
}

fn latest_assistant_text(agent: &Agent) -> Option<String> {
    latest_assistant_text_from_history(&agent.history)
}

fn serialize_plan_steps(steps: &[PlanStep]) -> Vec<Value> {
    steps
        .iter()
        .map(|step| {
            json!({
                "step": step.step,
                "status": match step.status {
                    crate::agent::PlanStepStatus::Pending => "pending",
                    crate::agent::PlanStepStatus::InProgress => "in_progress",
                    crate::agent::PlanStepStatus::Completed => "completed",
                }
            })
        })
        .collect()
}

fn serialize_pending_user_input(request: &PendingUserInput) -> Value {
    json!({
        "question": request.question,
        "options": request.options,
        "note": request.note,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        SubAgentKind, append_subagent_prompt, build_read_only_tool_manager,
        build_subagent_tool_manager, latest_assistant_text_from_history,
    };
    use crate::agent::Message;
    use crate::prompt::PromptRuntimeConfig;

    #[test]
    fn read_only_subagent_manager_excludes_mutating_and_agent_tools() {
        let manager = build_read_only_tool_manager();
        assert!(manager.get_tool("read_file").is_some());
        assert!(manager.get_tool("list_files").is_some());
        assert!(manager.get_tool("glob").is_some());
        assert!(manager.get_tool("grep").is_some());
        assert!(manager.get_tool("search_files").is_none());
        assert!(manager.get_tool("write_file").is_none());
        assert!(manager.get_tool("apply_patch").is_none());
        assert!(manager.get_tool("bash").is_none());
        assert!(manager.get_tool("background_task_list").is_none());
        assert!(manager.get_tool("background_task_status").is_none());
        assert!(manager.get_tool("background_task_stop").is_none());
        assert!(manager.get_tool("pty_start").is_none());
        assert!(manager.get_tool("pty_list").is_none());
        assert!(manager.get_tool("pty_status").is_none());
        assert!(manager.get_tool("pty_stop").is_none());
        assert!(manager.get_tool("spawn_agent").is_none());
        assert!(manager.get_tool("explore_agent").is_none());
        assert!(manager.get_tool("plan_agent").is_none());
        assert!(manager.get_tool("team_create").is_none());
    }

    #[test]
    fn general_subagent_manager_does_not_expose_recursive_agent_tools() {
        let manager = build_subagent_tool_manager(SubAgentKind::General);
        assert!(manager.get_tool("spawn_agent").is_none());
        assert!(manager.get_tool("explore_agent").is_none());
        assert!(manager.get_tool("plan_agent").is_none());
        assert!(manager.get_tool("team_create").is_none());
        assert!(manager.get_tool("bash").is_none());
        assert!(manager.get_tool("pty_start").is_none());
    }

    #[test]
    fn append_subagent_prompt_preserves_existing_append_prompt() {
        let runtime = PromptRuntimeConfig {
            append_system_prompt: Some("existing tail".to_string()),
            ..Default::default()
        };
        let updated = append_subagent_prompt(runtime, "sub-agent");
        assert_eq!(
            updated.append_system_prompt.as_deref(),
            Some("existing tail\n\nsub-agent")
        );
    }

    #[test]
    fn subagent_prompt_requires_instruction_constraints_and_workspace_boundary() {
        let prompt = SubAgentKind::Explore.append_prompt();

        assert!(prompt.contains("Treat the assigned instruction as the complete task contract."));
        assert!(prompt.contains("Honor every constraint in the assigned instruction"));
        assert!(prompt.contains("Stay inside the current workspace"));
    }

    #[test]
    fn general_subagent_prompt_declares_no_tool_access() {
        let prompt = SubAgentKind::General.append_prompt();

        assert!(prompt.contains("no-tool reasoning sub-agent"));
        assert!(
            prompt.contains("do not have repository, shell, editing, patching, or browser tools")
        );
        assert!(prompt.contains("answer only from the provided instruction/context"));
    }

    #[test]
    fn read_only_subagent_prompts_forbid_mutation_and_shell_workarounds() {
        for kind in [SubAgentKind::Explore, SubAgentKind::Plan] {
            let prompt = kind.append_prompt();

            assert!(prompt.contains("STRICT READ-ONLY"));
            assert!(prompt.contains("creating, modifying, deleting, moving, or copying files"));
            assert!(prompt.contains("including /tmp"));
            assert!(prompt.contains("redirection"));
            assert!(prompt.contains("Bash, PTY, editing, patching"));
            assert!(prompt.contains("read_file, list_files, glob, grep"));
            assert!(prompt.contains("instead of attempting a workaround"));
        }

        assert!(
            !SubAgentKind::General
                .append_prompt()
                .contains("STRICT READ-ONLY")
        );
    }

    #[test]
    fn latest_assistant_text_supports_string_content() {
        let history = vec![Message {
            role: "assistant".into(),
            content: json!("plain string assistant content"),
        }];

        assert_eq!(
            latest_assistant_text_from_history(&history).as_deref(),
            Some("plain string assistant content")
        );
    }
}
