use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::{self, StreamExt, TryStreamExt};
use rara_tool_macros::tool_spec;
use serde_json::{Value, json};

use crate::agent::{Agent, AgentExecutionMode, Message, PendingUserInput, PlanStep};
use crate::llm::LlmBackend;
use crate::prompt::PromptRuntimeConfig;
use crate::session::SessionManager;
use crate::session_transcript::{self, TranscriptScope};
use crate::tool::{Tool, ToolCallContext, ToolError, ToolManager};
use crate::tools::file::{ListFilesTool, ReadFileTool};
use crate::tools::search::{GlobTool, GrepTool};
use crate::vectordb::VectorDB;
use crate::workspace::WorkspaceMemory;

#[derive(Clone, Copy, Debug)]
enum SubAgentKind {
    General,
    Explore,
    Plan,
}

const TEAM_CREATE_MAX_TASKS: usize = 8;
const TEAM_CREATE_CONCURRENCY_LIMIT: usize = 4;

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

    fn label(self) -> &'static str {
        match self {
            SubAgentKind::General => "general",
            SubAgentKind::Explore => "explore",
            SubAgentKind::Plan => "plan",
        }
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
        self.call_with_parent_session(i, None).await
    }

    async fn call_with_context_events(
        &self,
        input: Value,
        context: ToolCallContext,
        _report: &mut (dyn FnMut(crate::tool::ToolProgressEvent) + Send),
    ) -> Result<Value, ToolError> {
        self.call_with_parent_session(input, context.session_id())
            .await
    }
}

impl AgentTool {
    async fn call_with_parent_session(
        &self,
        i: Value,
        parent_session_id: Option<&str>,
    ) -> Result<Value, ToolError> {
        let name = i["name"].as_str().unwrap_or("worker");
        let instruction = i["instruction"]
            .as_str()
            .ok_or(ToolError::InvalidInput("instruction".into()))?;
        let result = run_sub_agent(
            SubAgentKind::General,
            &next_subagent_id(SubAgentKind::General, Some(name)),
            parent_session_id,
            instruction,
            self.backend.clone(),
            self.vdb.clone(),
            self.session_manager.clone(),
            self.workspace.clone(),
            self.prompt_config.clone(),
        )
        .await?;
        Ok(json!({
            "agent_id": result.agent_id,
            "session_id": result.session_id,
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
        self.call_with_parent_session(i, None).await
    }

    async fn call_with_context_events(
        &self,
        input: Value,
        context: ToolCallContext,
        _report: &mut (dyn FnMut(crate::tool::ToolProgressEvent) + Send),
    ) -> Result<Value, ToolError> {
        self.call_with_parent_session(input, context.session_id())
            .await
    }
}

impl ExploreAgentTool {
    async fn call_with_parent_session(
        &self,
        i: Value,
        parent_session_id: Option<&str>,
    ) -> Result<Value, ToolError> {
        let instruction = i["instruction"]
            .as_str()
            .ok_or(ToolError::InvalidInput("instruction".into()))?;
        let result = run_sub_agent(
            SubAgentKind::Explore,
            &next_subagent_id(SubAgentKind::Explore, None),
            parent_session_id,
            instruction,
            self.backend.clone(),
            self.vdb.clone(),
            self.session_manager.clone(),
            self.workspace.clone(),
            self.prompt_config.clone(),
        )
        .await?;
        Ok(json!({
            "agent_id": result.agent_id,
            "session_id": result.session_id,
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
        self.call_with_parent_session(i, None).await
    }

    async fn call_with_context_events(
        &self,
        input: Value,
        context: ToolCallContext,
        _report: &mut (dyn FnMut(crate::tool::ToolProgressEvent) + Send),
    ) -> Result<Value, ToolError> {
        self.call_with_parent_session(input, context.session_id())
            .await
    }
}

impl PlanAgentTool {
    async fn call_with_parent_session(
        &self,
        i: Value,
        parent_session_id: Option<&str>,
    ) -> Result<Value, ToolError> {
        let instruction = i["instruction"]
            .as_str()
            .ok_or(ToolError::InvalidInput("instruction".into()))?;
        let result = run_sub_agent(
            SubAgentKind::Plan,
            &next_subagent_id(SubAgentKind::Plan, None),
            parent_session_id,
            instruction,
            self.backend.clone(),
            self.vdb.clone(),
            self.session_manager.clone(),
            self.workspace.clone(),
            self.prompt_config.clone(),
        )
        .await?;
        Ok(json!({
            "agent_id": result.agent_id,
            "session_id": result.session_id,
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
    pub prompt_config: PromptRuntimeConfig,
}

#[tool_spec(
    name = "team_create",
    description = "Launch up to 8 bounded sub-agents with at most 4 running concurrently. Each task must include a self-contained instruction and may set kind to general, explore, or plan.",
    input_schema = {
        "type": "object",
        "properties": {
            "tasks": {
                "type": "array",
                "maxItems": 8,
                "items": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "instruction": { "type": "string" },
                        "kind": {
                            "type": "string",
                            "enum": ["general", "explore", "plan"]
                        }
                    },
                    "required": ["instruction"]
                }
            }
        },
        "required": ["tasks"]
    }
)]
#[async_trait]
impl Tool for TeamCreateTool {
    async fn call(&self, i: Value) -> Result<Value, ToolError> {
        self.call_with_parent_session(i, None).await
    }

    async fn call_with_context_events(
        &self,
        input: Value,
        context: ToolCallContext,
        _report: &mut (dyn FnMut(crate::tool::ToolProgressEvent) + Send),
    ) -> Result<Value, ToolError> {
        self.call_with_parent_session(input, context.session_id())
            .await
    }
}

impl TeamCreateTool {
    async fn call_with_parent_session(
        &self,
        i: Value,
        parent_session_id: Option<&str>,
    ) -> Result<Value, ToolError> {
        let tasks = i["tasks"]
            .as_array()
            .ok_or(ToolError::InvalidInput("tasks".into()))?;
        if tasks.len() > TEAM_CREATE_MAX_TASKS {
            return Err(ToolError::InvalidInput(format!(
                "tasks must contain at most {TEAM_CREATE_MAX_TASKS} items"
            )));
        }

        let tasks = normalize_team_tasks(tasks)?;
        let runs = tasks.into_iter().map(|task| {
            let backend = self.backend.clone();
            let vdb = self.vdb.clone();
            let session_manager = self.session_manager.clone();
            let workspace = self.workspace.clone();
            let prompt_config = self.prompt_config.clone();
            let parent_session_id = parent_session_id.map(str::to_string);
            let agent_id = next_subagent_id(task.kind, Some(&task.name));

            async move {
                let result = run_sub_agent(
                    task.kind,
                    &agent_id,
                    parent_session_id.as_deref(),
                    &task.instruction,
                    backend,
                    vdb,
                    session_manager,
                    workspace,
                    prompt_config,
                )
                .await?;
                Ok::<_, ToolError>(serialize_team_result(&task.name, result))
            }
        });

        let results = stream::iter(runs)
            .buffered(TEAM_CREATE_CONCURRENCY_LIMIT)
            .try_collect::<Vec<_>>()
            .await?;
        Ok(json!({ "team_results": results }))
    }
}

struct TeamTask {
    name: String,
    instruction: String,
    kind: SubAgentKind,
}

struct SubAgentResult {
    agent_id: String,
    session_id: String,
    status: &'static str,
    summary: String,
    plan: Option<Vec<PlanStep>>,
    plan_explanation: Option<String>,
    request_user_input: Option<PendingUserInput>,
}

async fn run_sub_agent(
    kind: SubAgentKind,
    agent_id: &str,
    parent_session_id: Option<&str>,
    instruction: &str,
    backend: Arc<dyn LlmBackend>,
    vdb: Arc<VectorDB>,
    session_manager: Arc<SessionManager>,
    workspace: Arc<WorkspaceMemory>,
    prompt_config: PromptRuntimeConfig,
) -> Result<SubAgentResult, ToolError> {
    let tool_manager = build_subagent_tool_manager(kind);
    let mut sub = Agent::new(
        tool_manager,
        backend,
        vdb,
        session_manager.clone(),
        workspace,
    );
    sub.set_execution_mode(kind.execution_mode());
    sub.set_prompt_config(append_subagent_prompt(prompt_config, kind.append_prompt()));
    sub.query_with_mode(
        instruction.to_string(),
        crate::agent::AgentOutputMode::Silent,
    )
    .await
    .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

    if let Some(parent_session_id) = parent_session_id {
        write_subagent_sidechain(&session_manager, parent_session_id, agent_id, &sub)
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
    }

    Ok(SubAgentResult {
        agent_id: agent_id.to_string(),
        session_id: sub.session_id.clone(),
        status: kind.result_status(),
        summary: latest_assistant_text(&sub).unwrap_or_else(|| "Sub-agent finished.".to_string()),
        plan: (!sub.current_plan.is_empty()).then_some(sub.current_plan.clone()),
        plan_explanation: sub.plan_explanation.clone(),
        request_user_input: sub.pending_user_input.clone(),
    })
}

fn write_subagent_sidechain(
    session_manager: &SessionManager,
    parent_session_id: &str,
    agent_id: &str,
    sub: &Agent,
) -> anyhow::Result<()> {
    let path = session_transcript::subagent_transcript_path(
        &session_manager.storage_dir,
        parent_session_id,
        agent_id,
    );
    let scope = TranscriptScope::sidechain(parent_session_id, agent_id, sub.session_id.clone());
    session_transcript::write_message_snapshot(&path, &scope, &sub.history)
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

fn normalize_team_tasks(tasks: &[Value]) -> Result<Vec<TeamTask>, ToolError> {
    tasks
        .iter()
        .enumerate()
        .map(|(idx, task)| {
            let name = task["name"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| format!("worker-{}", idx + 1));
            let instruction = task["instruction"]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| ToolError::InvalidInput(format!("tasks[{idx}].instruction")))?;
            let kind = match task.get("kind") {
                Some(value) => {
                    let kind = value.as_str().ok_or_else(|| {
                        ToolError::InvalidInput(format!("tasks[{idx}].kind must be a string"))
                    })?;
                    parse_team_task_kind(idx, Some(kind))?
                }
                None => parse_team_task_kind(idx, None)?,
            };
            Ok(TeamTask {
                name,
                instruction,
                kind,
            })
        })
        .collect()
}

fn parse_team_task_kind(idx: usize, kind: Option<&str>) -> Result<SubAgentKind, ToolError> {
    match kind.unwrap_or("explore") {
        "general" => Ok(SubAgentKind::General),
        "explore" => Ok(SubAgentKind::Explore),
        "plan" => Ok(SubAgentKind::Plan),
        other => Err(ToolError::InvalidInput(format!(
            "tasks[{idx}].kind must be one of general, explore, or plan; got {other}"
        ))),
    }
}

fn serialize_team_result(name: &str, result: SubAgentResult) -> Value {
    json!({
        "agent_id": result.agent_id,
        "session_id": result.session_id,
        "name": name,
        "status": result.status,
        "summary": result.summary,
        "plan": result.plan.as_ref().map(|steps| serialize_plan_steps(steps)),
        "plan_explanation": result.plan_explanation,
        "request_user_input": result
            .request_user_input
            .as_ref()
            .map(serialize_pending_user_input),
    })
}

fn next_subagent_id(kind: SubAgentKind, name: Option<&str>) -> String {
    let label = name
        .map(sanitize_agent_id_part)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| kind.label().to_string());
    format!("{label}-{}", uuid::Uuid::new_v4())
}

fn sanitize_agent_id_part(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
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
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use async_trait::async_trait;
    use serde_json::json;
    use tempfile::tempdir;
    use tokio::time::{Duration, sleep};

    use super::{
        SubAgentKind, TEAM_CREATE_CONCURRENCY_LIMIT, append_subagent_prompt,
        build_read_only_tool_manager, build_subagent_tool_manager,
        latest_assistant_text_from_history, parse_team_task_kind,
    };
    use crate::agent::Message;
    use crate::llm::{ContentBlock, LlmBackend, LlmResponse, MockLlm};
    use crate::prompt::PromptRuntimeConfig;
    use crate::session::SessionManager;
    use crate::session_transcript::{load_transcript, model_visible_messages};
    use crate::tool::{Tool, ToolCallContext, ToolError};
    use crate::tools::agent::{ExploreAgentTool, TeamCreateTool};
    use crate::vectordb::VectorDB;
    use crate::workspace::WorkspaceMemory;

    struct CountingBackend {
        calls: Arc<AtomicUsize>,
    }

    struct PeakBackend {
        in_flight: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
    }

    fn record_peak(current: usize, peak: &AtomicUsize) {
        let mut observed = peak.load(Ordering::SeqCst);
        while current > observed {
            match peak.compare_exchange(observed, current, Ordering::SeqCst, Ordering::SeqCst) {
                Ok(_) => break,
                Err(next) => observed = next,
            }
        }
    }

    #[async_trait]
    impl LlmBackend for CountingBackend {
        async fn ask(
            &self,
            messages: &[Message],
            _tools: &[serde_json::Value],
        ) -> anyhow::Result<LlmResponse> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let last = messages
                .last()
                .and_then(|message| message.content.as_str())
                .unwrap_or_default();
            Ok(LlmResponse {
                content: vec![ContentBlock::Text {
                    text: format!("counted {last}"),
                }],
                stop_reason: Some("end_turn".to_string()),
                usage: None,
            })
        }

        async fn embed(&self, _text: &str) -> anyhow::Result<Vec<f32>> {
            Ok(vec![0.0; 4])
        }

        async fn summarize(
            &self,
            _messages: &[Message],
            _instruction: &str,
        ) -> anyhow::Result<String> {
            Ok("summary".to_string())
        }
    }

    #[async_trait]
    impl LlmBackend for PeakBackend {
        async fn ask(
            &self,
            messages: &[Message],
            _tools: &[serde_json::Value],
        ) -> anyhow::Result<LlmResponse> {
            let current = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            record_peak(current, &self.peak);
            sleep(Duration::from_millis(50)).await;
            self.in_flight.fetch_sub(1, Ordering::SeqCst);
            let last = messages
                .last()
                .and_then(|message| message.content.as_str())
                .unwrap_or_default();
            Ok(LlmResponse {
                content: vec![ContentBlock::Text {
                    text: format!("peak {last}"),
                }],
                stop_reason: Some("end_turn".to_string()),
                usage: None,
            })
        }

        async fn embed(&self, _text: &str) -> anyhow::Result<Vec<f32>> {
            Ok(vec![0.0; 4])
        }

        async fn summarize(
            &self,
            _messages: &[Message],
            _instruction: &str,
        ) -> anyhow::Result<String> {
            Ok("summary".to_string())
        }
    }

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

    #[test]
    fn team_task_kind_defaults_to_explore_and_rejects_unknown_values() {
        assert!(matches!(
            parse_team_task_kind(0, None).unwrap(),
            SubAgentKind::Explore
        ));
        assert!(matches!(
            parse_team_task_kind(0, Some("general")).unwrap(),
            SubAgentKind::General
        ));
        assert!(matches!(
            parse_team_task_kind(0, Some("plan")).unwrap(),
            SubAgentKind::Plan
        ));
        let err = parse_team_task_kind(3, Some("unknown")).expect_err("invalid kind");
        assert!(
            matches!(err, ToolError::InvalidInput(message) if message.contains("tasks[3].kind"))
        );
    }

    #[tokio::test]
    async fn team_create_runs_real_subagents_in_order() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path().join("workspace");
        let rara_dir = temp.path().join(".rara");
        std::fs::create_dir_all(&root).expect("workspace");
        let tool = TeamCreateTool {
            backend: Arc::new(MockLlm),
            vdb: Arc::new(VectorDB::new(
                &rara_dir.join("lancedb").display().to_string(),
            )),
            session_manager: Arc::new(
                SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
            ),
            workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir)),
            prompt_config: PromptRuntimeConfig::default(),
        };

        let result = tool
            .call(json!({
                "tasks": [
                    {
                        "name": "research",
                        "kind": "general",
                        "instruction": "summarize one"
                    },
                    {
                        "name": "inspect",
                        "kind": "explore",
                        "instruction": "summarize two"
                    }
                ]
            }))
            .await
            .expect("team_create result");
        let results = result["team_results"]
            .as_array()
            .expect("team_results array");

        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["name"], "research");
        assert_eq!(results[0]["status"], "done");
        assert_eq!(results[0]["summary"], "Mock Response: summarize one");
        assert_eq!(results[1]["name"], "inspect");
        assert_eq!(results[1]["status"], "explored");
        assert_eq!(results[1]["summary"], "Mock Response: summarize two");
        assert_ne!(results[0]["status"], "mocked_done");
    }

    #[tokio::test]
    async fn team_create_validates_all_tasks_before_running_subagents() {
        let calls = Arc::new(AtomicUsize::new(0));
        let temp = tempdir().expect("tempdir");
        let root = temp.path().join("workspace");
        let rara_dir = temp.path().join(".rara");
        std::fs::create_dir_all(&root).expect("workspace");
        let tool = TeamCreateTool {
            backend: Arc::new(CountingBackend {
                calls: calls.clone(),
            }),
            vdb: Arc::new(VectorDB::new(
                &rara_dir.join("lancedb").display().to_string(),
            )),
            session_manager: Arc::new(
                SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
            ),
            workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir)),
            prompt_config: PromptRuntimeConfig::default(),
        };

        let err = tool
            .call(json!({
                "tasks": [
                    {
                        "name": "valid",
                        "instruction": "should not run"
                    },
                    {
                        "name": "invalid"
                    }
                ]
            }))
            .await
            .expect_err("invalid task");

        assert!(
            matches!(err, ToolError::InvalidInput(message) if message == "tasks[1].instruction")
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn team_create_rejects_non_string_kind_before_running_subagents() {
        let calls = Arc::new(AtomicUsize::new(0));
        let temp = tempdir().expect("tempdir");
        let root = temp.path().join("workspace");
        let rara_dir = temp.path().join(".rara");
        std::fs::create_dir_all(&root).expect("workspace");
        let tool = TeamCreateTool {
            backend: Arc::new(CountingBackend {
                calls: calls.clone(),
            }),
            vdb: Arc::new(VectorDB::new(
                &rara_dir.join("lancedb").display().to_string(),
            )),
            session_manager: Arc::new(
                SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
            ),
            workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir)),
            prompt_config: PromptRuntimeConfig::default(),
        };

        let err = tool
            .call(json!({
                "tasks": [
                    {
                        "instruction": "should not run",
                        "kind": 1
                    }
                ]
            }))
            .await
            .expect_err("invalid kind");

        assert!(
            matches!(err, ToolError::InvalidInput(message) if message == "tasks[0].kind must be a string")
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn team_create_limits_concurrent_subagents() {
        let in_flight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let temp = tempdir().expect("tempdir");
        let root = temp.path().join("workspace");
        let rara_dir = temp.path().join(".rara");
        std::fs::create_dir_all(&root).expect("workspace");
        let tool = TeamCreateTool {
            backend: Arc::new(PeakBackend {
                in_flight,
                peak: peak.clone(),
            }),
            vdb: Arc::new(VectorDB::new(
                &rara_dir.join("lancedb").display().to_string(),
            )),
            session_manager: Arc::new(
                SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
            ),
            workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir)),
            prompt_config: PromptRuntimeConfig::default(),
        };
        let tasks = (0..8)
            .map(|idx| json!({ "kind": "general", "instruction": format!("task {idx}") }))
            .collect::<Vec<_>>();

        let result = tool
            .call(json!({ "tasks": tasks }))
            .await
            .expect("team_create result");

        assert_eq!(result["team_results"].as_array().expect("results").len(), 8);
        let observed_peak = peak.load(Ordering::SeqCst);
        assert!(observed_peak <= TEAM_CREATE_CONCURRENCY_LIMIT);
        assert!(observed_peak > 1);
    }

    #[tokio::test]
    async fn team_create_writes_parent_scoped_sidechain_transcripts() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path().join("workspace");
        let rara_dir = temp.path().join(".rara");
        std::fs::create_dir_all(&root).expect("workspace");
        let tool = TeamCreateTool {
            backend: Arc::new(CountingBackend {
                calls: Arc::new(AtomicUsize::new(0)),
            }),
            vdb: Arc::new(VectorDB::new(
                &rara_dir.join("lancedb").display().to_string(),
            )),
            session_manager: Arc::new(
                SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
            ),
            workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir.clone())),
            prompt_config: PromptRuntimeConfig::default(),
        };

        let mut progress = |_| {};
        let result = tool
            .call_with_context_events(
                json!({
                    "tasks": [
                        {
                            "name": "Review Worker",
                            "kind": "general",
                            "instruction": "summarize this task"
                        }
                    ]
                }),
                ToolCallContext::default().with_session_id("parent-session"),
                &mut progress,
            )
            .await
            .expect("team_create result");

        let item = &result["team_results"][0];
        let agent_id = item["agent_id"].as_str().expect("agent_id");
        let child_session_id = item["session_id"].as_str().expect("session_id");
        assert!(agent_id.starts_with("review-worker-"));
        let transcript_path = rara_dir
            .join("rollouts")
            .join("parent-session")
            .join("subagents")
            .join(format!("agent-{agent_id}.jsonl"));
        let transcript = load_transcript(&transcript_path).expect("transcript");

        assert_eq!(transcript.parse_errors, 0);
        assert!(model_visible_messages(&transcript.entries).is_empty());
        assert!(matches!(
            &transcript.entries[0],
            crate::session_transcript::SessionTranscriptEntry::SessionMeta {
                session_id,
                parent_session_id: Some(parent),
                agent_id: Some(entry_agent_id),
                is_sidechain: true,
                ..
            } if session_id == child_session_id
                && parent == "parent-session"
                && entry_agent_id == agent_id
        ));
    }

    #[tokio::test]
    async fn subagent_without_parent_context_does_not_write_sidechain() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path().join("workspace");
        let rara_dir = temp.path().join(".rara");
        std::fs::create_dir_all(&root).expect("workspace");
        let tool = ExploreAgentTool {
            backend: Arc::new(CountingBackend {
                calls: Arc::new(AtomicUsize::new(0)),
            }),
            vdb: Arc::new(VectorDB::new(
                &rara_dir.join("lancedb").display().to_string(),
            )),
            session_manager: Arc::new(
                SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
            ),
            workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir.clone())),
            prompt_config: PromptRuntimeConfig::default(),
        };

        let result = tool
            .call(json!({ "instruction": "look around" }))
            .await
            .expect("explore result");

        assert!(
            result["agent_id"]
                .as_str()
                .expect("agent_id")
                .starts_with("explore-")
        );
        assert!(!rara_dir.join("rollouts").join("subagents").exists());
    }

    #[tokio::test]
    async fn team_create_rejects_too_many_tasks() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path().join("workspace");
        let rara_dir = temp.path().join(".rara");
        std::fs::create_dir_all(&root).expect("workspace");
        let tool = TeamCreateTool {
            backend: Arc::new(MockLlm),
            vdb: Arc::new(VectorDB::new(
                &rara_dir.join("lancedb").display().to_string(),
            )),
            session_manager: Arc::new(
                SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
            ),
            workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir)),
            prompt_config: PromptRuntimeConfig::default(),
        };

        let tasks = (0..9)
            .map(|idx| json!({ "instruction": format!("task {idx}") }))
            .collect::<Vec<_>>();
        let err = tool
            .call(json!({ "tasks": tasks }))
            .await
            .expect_err("too many tasks");

        assert!(
            matches!(err, ToolError::InvalidInput(message) if message.contains("at most 8 items"))
        );
    }
}
