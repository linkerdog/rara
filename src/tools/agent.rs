use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use futures::stream::{self, StreamExt, TryStreamExt};
use rara_memory::vectordb::VectorDB;
use rara_state::state_db::{
    PersistedCompactState, PersistedInteraction, PersistedPlanStep, PersistedPromptRuntimeState,
    StateDb,
};
use rara_tool_macros::tool_spec;
use rara_tools::file::{ListFilesTool, ReadFileTool};
use rara_tools::search::{GlobTool, GrepTool};
use rara_tools::tool::{Tool, ToolCallContext, ToolError, ToolManager};
use serde_json::{Value, json};

use crate::agent::{
    Agent, AgentExecutionMode, Message, PendingUserInput, PlanStep, PlanStepStatus,
};
use crate::llm::LlmBackend;
use crate::prompt::PromptRuntimeConfig;
use crate::session::SessionManager;
use crate::session_transcript::{self, TranscriptScope};
use crate::thread_store::{ThreadRecorder, ThreadRuntimeLineage, ThreadRuntimeState};
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

macro_rules! subagent_search_guidelines {
    () => {
        concat!(
            "## Search & Analysis Guidelines\n",
            "- For searches: search broadly with glob/grep when you don't know where something lives. Use read_file when you know the specific file path.\n",
            "- For analysis: start broad and narrow down. Use multiple search strategies if the first doesn't yield results.\n",
            "- Be thorough: check multiple locations, consider different naming conventions, look for related files.\n",
            "- Prefer 'rg' for text search and 'rg --files' for file discovery — faster than grep/find.\n",
            "- Do not repeat the same discovery tool call with the same arguments unless the workspace changed.\n",
            "- When tracing behavior, follow one complete path from entry point to side effect before branching into other paths.\n",
            "- Do not narrate each next tool call; let the tool transcript show inspection steps.",
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
                    subagent_search_guidelines!(),
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
                    subagent_search_guidelines!(),
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
    pub background_subagents: Arc<BackgroundSubAgentStore>,
}

#[tool_spec(
    name = "spawn_agent",
    description = "Spawn a no-tool reasoning sub-agent. It cannot inspect files, run shell commands, edit files, or spawn other agents; use explore_agent or plan_agent for read-only repository inspection.",
    input_schema = {
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "instruction": { "type": "string" },
            "run_in_background": {
                "type": "boolean",
                "default": false,
                "description": "Start the sub-agent in the background and return immediately. Use subagent_resume to inspect the result and subagent_stop to cancel a running sub-agent."
            }
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
        _report: &mut (dyn FnMut(rara_tools::tool::ToolProgressEvent) + Send),
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
        let name = i["name"]
            .as_str()
            .ok_or(ToolError::InvalidInput("name".into()))?;
        if validate_agent_id_label(name).is_none() {
            return Err(ToolError::InvalidInput(
                "name must normalize to a non-empty agent id label".into(),
            ));
        }
        let instruction = i["instruction"]
            .as_str()
            .ok_or(ToolError::InvalidInput("instruction".into()))?;
        let agent_id = next_subagent_id(SubAgentKind::General, Some(name));
        if i.get("run_in_background")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let task = self.background_subagents.start(BackgroundSubAgentStart {
                kind: SubAgentKind::General,
                agent_id,
                name: Some(name.to_string()),
                parent_session_id: parent_session_id.map(str::to_string),
                instruction: instruction.to_string(),
                backend: self.backend.clone(),
                vdb: self.vdb.clone(),
                session_manager: self.session_manager.clone(),
                workspace: self.workspace.clone(),
                prompt_config: self.prompt_config.clone(),
            })?;
            return Ok(task.to_json());
        }
        let result = run_sub_agent(
            SubAgentKind::General,
            &agent_id,
            Some(name),
            parent_session_id,
            instruction,
            None,
            None,
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
            "persistence_error": result.persistence_error,
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
    pub background_subagents: Arc<BackgroundSubAgentStore>,
}

#[tool_spec(
    name = "explore_agent",
    description = "Spawn a read-only exploration sub-agent for bounded independent sidecar repository inspection. The instruction must be self-contained and include all user constraints.",
    input_schema = {
        "type": "object",
        "properties": {
            "instruction": { "type": "string" },
            "run_in_background": {
                "type": "boolean",
                "default": false,
                "description": "Start the exploration sub-agent in the background and return immediately. Use subagent_resume to inspect the result and subagent_stop to cancel a running sub-agent."
            }
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
        _report: &mut (dyn FnMut(rara_tools::tool::ToolProgressEvent) + Send),
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
        let agent_id = next_subagent_id(SubAgentKind::Explore, None);
        if i.get("run_in_background")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let task = self.background_subagents.start(BackgroundSubAgentStart {
                kind: SubAgentKind::Explore,
                agent_id,
                name: None,
                parent_session_id: parent_session_id.map(str::to_string),
                instruction: instruction.to_string(),
                backend: self.backend.clone(),
                vdb: self.vdb.clone(),
                session_manager: self.session_manager.clone(),
                workspace: self.workspace.clone(),
                prompt_config: self.prompt_config.clone(),
            })?;
            return Ok(task.to_json());
        }
        let result = run_sub_agent(
            SubAgentKind::Explore,
            &agent_id,
            None,
            parent_session_id,
            instruction,
            None,
            None,
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
            "persistence_error": result.persistence_error,
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
    pub background_subagents: Arc<BackgroundSubAgentStore>,
}

#[tool_spec(
    name = "plan_agent",
    description = "Spawn a read-only planning sub-agent for bounded independent sidecar plan refinement. The instruction must be self-contained and include all user constraints.",
    input_schema = {
        "type": "object",
        "properties": {
            "instruction": { "type": "string" },
            "run_in_background": {
                "type": "boolean",
                "default": false,
                "description": "Start the planning sub-agent in the background and return immediately. Use subagent_resume to inspect the result and subagent_stop to cancel a running sub-agent."
            }
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
        _report: &mut (dyn FnMut(rara_tools::tool::ToolProgressEvent) + Send),
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
        let agent_id = next_subagent_id(SubAgentKind::Plan, None);
        if i.get("run_in_background")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let task = self.background_subagents.start(BackgroundSubAgentStart {
                kind: SubAgentKind::Plan,
                agent_id,
                name: None,
                parent_session_id: parent_session_id.map(str::to_string),
                instruction: instruction.to_string(),
                backend: self.backend.clone(),
                vdb: self.vdb.clone(),
                session_manager: self.session_manager.clone(),
                workspace: self.workspace.clone(),
                prompt_config: self.prompt_config.clone(),
            })?;
            return Ok(task.to_json());
        }
        let result = run_sub_agent(
            SubAgentKind::Plan,
            &agent_id,
            None,
            parent_session_id,
            instruction,
            None,
            None,
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
            "persistence_error": result.persistence_error,
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

const BACKGROUND_SUBAGENT_COMPLETED_RETENTION: usize = 64;

#[derive(Clone, Default)]
pub struct BackgroundSubAgentStore {
    inner: Arc<Mutex<BackgroundSubAgentState>>,
}

#[derive(Default)]
struct BackgroundSubAgentState {
    tasks: HashMap<String, BackgroundSubAgentRecord>,
    cancellations: HashMap<String, Arc<AtomicBool>>,
}

struct BackgroundSubAgentStart {
    kind: SubAgentKind,
    agent_id: String,
    name: Option<String>,
    parent_session_id: Option<String>,
    instruction: String,
    backend: Arc<dyn LlmBackend>,
    vdb: Arc<VectorDB>,
    session_manager: Arc<SessionManager>,
    workspace: Arc<WorkspaceMemory>,
    prompt_config: PromptRuntimeConfig,
}

#[derive(Clone, Debug)]
struct BackgroundSubAgentRecord {
    agent_id: String,
    session_id: String,
    name: Option<String>,
    kind: &'static str,
    parent_session_id: Option<String>,
    status: &'static str,
    summary: Option<String>,
    error: Option<String>,
    persistence_error: Option<String>,
    plan: Option<Vec<PlanStep>>,
    plan_explanation: Option<String>,
    request_user_input: Option<PendingUserInput>,
    started_at: u64,
    finished_at: Option<u64>,
}

impl BackgroundSubAgentStore {
    fn start(&self, start: BackgroundSubAgentStart) -> Result<BackgroundSubAgentRecord, ToolError> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let cancellation = Arc::new(AtomicBool::new(false));
        let record = BackgroundSubAgentRecord {
            agent_id: start.agent_id.clone(),
            session_id: session_id.clone(),
            name: start.name.clone(),
            kind: start.kind.label(),
            parent_session_id: start.parent_session_id.clone(),
            status: "running",
            summary: None,
            error: None,
            persistence_error: None,
            plan: None,
            plan_explanation: None,
            request_user_input: None,
            started_at: unix_timestamp_secs(),
            finished_at: None,
        };
        {
            let mut inner = self
                .inner
                .lock()
                .map_err(|_| ToolError::ExecutionFailed("sub-agent store poisoned".into()))?;
            if inner.tasks.contains_key(&start.agent_id) {
                return Err(ToolError::InvalidInput(format!(
                    "duplicate sub-agent id: {}",
                    start.agent_id
                )));
            }
            inner.tasks.insert(start.agent_id.clone(), record.clone());
            inner
                .cancellations
                .insert(start.agent_id.clone(), cancellation.clone());
        }

        let store = self.clone();
        let agent_id = start.agent_id.clone();
        tokio::spawn(async move {
            let result = run_sub_agent(
                start.kind,
                &start.agent_id,
                start.name.as_deref(),
                start.parent_session_id.as_deref(),
                &start.instruction,
                Some(session_id),
                Some(cancellation),
                start.backend,
                start.vdb,
                start.session_manager,
                start.workspace,
                start.prompt_config,
            )
            .await;
            store.finish(&agent_id, result);
        });

        Ok(record)
    }

    fn get(&self, agent_id: &str) -> Result<BackgroundSubAgentRecord, ToolError> {
        self.inner
            .lock()
            .map_err(|_| ToolError::ExecutionFailed("sub-agent store poisoned".into()))?
            .tasks
            .get(agent_id)
            .cloned()
            .ok_or_else(|| ToolError::InvalidInput(format!("unknown sub-agent id: {agent_id}")))
    }

    fn list(&self) -> Result<Vec<BackgroundSubAgentRecord>, ToolError> {
        let records = self
            .inner
            .lock()
            .map_err(|_| ToolError::ExecutionFailed("sub-agent store poisoned".into()))?
            .tasks
            .values()
            .cloned()
            .collect::<Vec<_>>();
        Ok(records)
    }

    fn stop(&self, agent_id: &str) -> Result<BackgroundSubAgentRecord, ToolError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| ToolError::ExecutionFailed("sub-agent store poisoned".into()))?;
        let record = inner
            .tasks
            .get_mut(agent_id)
            .ok_or_else(|| ToolError::InvalidInput(format!("unknown sub-agent id: {agent_id}")))?;
        if record.status != "running" {
            return Ok(record.clone());
        }
        record.status = "cancelled";
        record.finished_at = Some(unix_timestamp_secs());
        let stopped = record.clone();
        let token = inner.cancellations.remove(agent_id);
        prune_completed_subagents(&mut inner, Some(agent_id));
        drop(inner);

        if let Some(token) = token {
            token.store(true, Ordering::SeqCst);
        }
        Ok(stopped)
    }

    fn finish(&self, agent_id: &str, result: Result<SubAgentResult, ToolError>) {
        let Ok(mut inner) = self.inner.lock() else {
            eprintln!("sub-agent store poisoned while finishing {agent_id}");
            return;
        };
        inner.cancellations.remove(agent_id);
        let Some(record) = inner.tasks.get_mut(agent_id) else {
            return;
        };
        if record.status == "cancelled" {
            return;
        }
        record.finished_at = Some(unix_timestamp_secs());
        match result {
            Ok(result) => {
                record.status = result.status;
                record.summary = Some(result.summary);
                record.persistence_error = result.persistence_error;
                record.plan = result.plan;
                record.plan_explanation = result.plan_explanation;
                record.request_user_input = result.request_user_input;
                record.error = None;
            }
            Err(err) => {
                record.status = "failed";
                record.error = Some(err.to_string());
            }
        }
        prune_completed_subagents(&mut inner, Some(agent_id));
    }
}

fn prune_completed_subagents(inner: &mut BackgroundSubAgentState, preserve_agent_id: Option<&str>) {
    let completed_count = inner
        .tasks
        .values()
        .filter(|record| record.finished_at.is_some())
        .count();
    if completed_count <= BACKGROUND_SUBAGENT_COMPLETED_RETENTION {
        return;
    }

    let mut candidates = inner
        .tasks
        .values()
        .filter(|record| {
            record.finished_at.is_some() && Some(record.agent_id.as_str()) != preserve_agent_id
        })
        .map(|record| {
            (
                record.agent_id.clone(),
                record.finished_at.unwrap_or(u64::MAX),
                record.started_at,
            )
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.1.cmp(&right.1).then(left.2.cmp(&right.2)));

    let remove_count = completed_count.saturating_sub(BACKGROUND_SUBAGENT_COMPLETED_RETENTION);
    for (agent_id, _, _) in candidates.into_iter().take(remove_count) {
        inner.tasks.remove(&agent_id);
        inner.cancellations.remove(&agent_id);
    }
}

impl BackgroundSubAgentRecord {
    fn to_json(&self) -> Value {
        json!({
            "agent_id": self.agent_id,
            "session_id": self.session_id,
            "name": self.name,
            "kind": self.kind,
            "parent_session_id": self.parent_session_id,
            "status": self.status,
            "summary": self.summary,
            "error": self.error,
            "persistence_error": self.persistence_error,
            "plan": self.plan.as_ref().map(|steps| serialize_plan_steps(steps)),
            "plan_explanation": self.plan_explanation,
            "request_user_input": self
                .request_user_input
                .as_ref()
                .map(serialize_pending_user_input),
            "started_at": self.started_at,
            "finished_at": self.finished_at,
        })
    }
}

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

pub struct SubAgentResumeTool {
    pub background_subagents: Arc<BackgroundSubAgentStore>,
}

#[tool_spec(
    name = "subagent_resume",
    description = "Resume observing a background sub-agent by agent_id. Returns the running status or the completed result summary without reading the sidechain transcript into parent context.",
    input_schema = {
        "type": "object",
        "properties": {
            "agent_id": {
                "type": "string",
                "description": "Background sub-agent id returned by spawn_agent, explore_agent, or plan_agent when run_in_background is true."
            }
        },
        "required": ["agent_id"]
    }
)]
#[async_trait]
impl Tool for SubAgentResumeTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        let agent_id = input["agent_id"]
            .as_str()
            .ok_or(ToolError::InvalidInput("agent_id".into()))?;
        Ok(self.background_subagents.get(agent_id)?.to_json())
    }
}

pub struct SubAgentListTool {
    pub background_subagents: Arc<BackgroundSubAgentStore>,
}

#[tool_spec(
    name = "subagent_list",
    description = "List in-process background sub-agents for this RARA runtime. Completed sidechain transcripts remain on disk; this list is for live background control.",
    input_schema = {
        "type": "object",
        "properties": {}
    }
)]
#[async_trait]
impl Tool for SubAgentListTool {
    async fn call(&self, _input: Value) -> Result<Value, ToolError> {
        let mut agents = self
            .background_subagents
            .list()?
            .into_iter()
            .collect::<Vec<_>>();
        agents.sort_by(|left, right| {
            left.started_at
                .cmp(&right.started_at)
                .then(left.agent_id.cmp(&right.agent_id))
        });
        let agents = agents
            .into_iter()
            .map(|record| record.to_json())
            .collect::<Vec<_>>();
        Ok(json!({ "subagents": agents }))
    }
}

pub struct SubAgentStopTool {
    pub background_subagents: Arc<BackgroundSubAgentStore>,
}

#[tool_spec(
    name = "subagent_stop",
    description = "Request cancellation for a running background sub-agent by agent_id. The sidechain contract remains parent-scoped; this does not inject the child transcript into parent context.",
    input_schema = {
        "type": "object",
        "properties": {
            "agent_id": {
                "type": "string",
                "description": "Background sub-agent id returned by spawn_agent, explore_agent, or plan_agent when run_in_background is true."
            }
        },
        "required": ["agent_id"]
    }
)]
#[async_trait]
impl Tool for SubAgentStopTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        let agent_id = input["agent_id"]
            .as_str()
            .ok_or(ToolError::InvalidInput("agent_id".into()))?;
        Ok(self.background_subagents.stop(agent_id)?.to_json())
    }
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
        _report: &mut (dyn FnMut(rara_tools::tool::ToolProgressEvent) + Send),
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
                    Some(&task.name),
                    parent_session_id.as_deref(),
                    &task.instruction,
                    None,
                    None,
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
    persistence_error: Option<String>,
    plan: Option<Vec<PlanStep>>,
    plan_explanation: Option<String>,
    request_user_input: Option<PendingUserInput>,
}

async fn run_sub_agent(
    kind: SubAgentKind,
    agent_id: &str,
    name: Option<&str>,
    parent_session_id: Option<&str>,
    instruction: &str,
    session_id: Option<String>,
    cancellation_token: Option<Arc<AtomicBool>>,
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
        workspace.clone(),
    );
    if let Some(session_id) = session_id {
        sub.session_id = session_id;
    }
    sub.set_cancellation_token(cancellation_token);
    sub.set_execution_mode(kind.execution_mode());
    sub.set_prompt_config(append_subagent_prompt(prompt_config, kind.append_prompt()));
    sub.query_with_mode(
        instruction.to_string(),
        crate::agent::AgentOutputMode::Silent,
    )
    .await
    .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

    let status = kind.result_status();
    let summary = latest_assistant_text(&sub).unwrap_or_else(|| "Sub-agent finished.".to_string());

    let persistence_error = parent_session_id.and_then(|parent_session_id| {
        persist_subagent_edge(
            &session_manager,
            &workspace,
            parent_session_id,
            agent_id,
            name,
            &sub,
            status,
            &summary,
        )
        .err()
        .map(|err| err.to_string())
    });

    Ok(SubAgentResult {
        agent_id: agent_id.to_string(),
        session_id: sub.session_id.clone(),
        status,
        summary,
        persistence_error,
        plan: (!sub.current_plan.is_empty()).then_some(sub.current_plan.clone()),
        plan_explanation: sub.plan_explanation.clone(),
        request_user_input: sub.pending_user_input.clone(),
    })
}

fn persist_subagent_edge(
    session_manager: &SessionManager,
    workspace: &WorkspaceMemory,
    parent_session_id: &str,
    agent_id: &str,
    name: Option<&str>,
    sub: &Agent,
    status: &str,
    summary: &str,
) -> anyhow::Result<()> {
    write_subagent_sidechain(session_manager, parent_session_id, agent_id, sub)?;
    persist_subagent_runtime_state(session_manager, workspace, parent_session_id, sub)?;
    session_manager.save_spawn_agent_event(
        parent_session_id,
        &format!("spawn-{}", uuid::Uuid::new_v4()),
        agent_id,
        name,
        &sub.session_id,
        status,
        Some(summary),
    )
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

fn persist_subagent_runtime_state(
    session_manager: &SessionManager,
    workspace: &WorkspaceMemory,
    parent_session_id: &str,
    sub: &Agent,
) -> anyhow::Result<()> {
    let rara_dir = session_manager
        .storage_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("session storage dir has no parent"))?;
    let state_db = StateDb::new_for_root_dir(rara_dir.to_path_buf())?;
    let recorder = ThreadRecorder::new(&state_db);
    let (cwd, branch) = workspace.get_env_info();

    recorder.persist_runtime_state_with_lineage(
        &ThreadRuntimeState {
            session_id: &sub.session_id,
            cwd: &cwd,
            branch: &branch,
            provider: "subagent",
            model: "subagent",
            base_url: None,
            agent_mode: sub.execution_mode_label(),
            bash_approval: "unavailable",
            plan_explanation: sub.plan_explanation.as_deref(),
            prompt_runtime: PersistedPromptRuntimeState::default(),
            history_len: sub.history.len(),
            transcript_len: sub.history.len(),
            compact_state: PersistedCompactState::default(),
        },
        &ThreadRuntimeLineage {
            origin_kind: "subagent".to_string(),
            forked_from_thread_id: Some(parent_session_id.to_string()),
        },
    )?;

    recorder.replace_plan_steps(&sub.session_id, &persisted_plan_steps(&sub.current_plan))?;
    recorder.replace_interactions(
        &sub.session_id,
        &persisted_pending_interactions(sub.pending_user_input.as_ref()),
    )?;

    Ok(())
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
            let name = normalize_team_task_name(idx, task)?;
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

fn normalize_team_task_name(idx: usize, task: &Value) -> Result<String, ToolError> {
    match task.get("name") {
        Some(value) => {
            let name = value.as_str().ok_or_else(|| {
                ToolError::InvalidInput(format!("tasks[{idx}].name must be a string"))
            })?;
            if validate_agent_id_label(name).is_none() {
                return Err(ToolError::InvalidInput(format!(
                    "tasks[{idx}].name must normalize to a non-empty agent id label"
                )));
            }
            Ok(name.to_string())
        }
        None => Ok(format!("worker-{}", idx + 1)),
    }
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
        "persistence_error": result.persistence_error,
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
        .and_then(validate_agent_id_label)
        .unwrap_or_else(|| kind.label().to_string());
    format!("{label}-{}", uuid::Uuid::new_v4())
}

fn validate_agent_id_label(value: &str) -> Option<String> {
    let label = value
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
        .to_string();
    (!label.is_empty()).then_some(label)
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
                "status": plan_step_status_label(&step.status),
            })
        })
        .collect()
}

fn persisted_plan_steps(steps: &[PlanStep]) -> Vec<PersistedPlanStep> {
    steps
        .iter()
        .enumerate()
        .map(|(step_index, step)| PersistedPlanStep {
            step_index,
            status: plan_step_status_label(&step.status).to_string(),
            step: step.step.clone(),
        })
        .collect()
}

fn plan_step_status_label(status: &PlanStepStatus) -> &'static str {
    match status {
        PlanStepStatus::Pending => "pending",
        PlanStepStatus::InProgress => "in_progress",
        PlanStepStatus::Completed => "completed",
    }
}

fn serialize_pending_user_input(request: &PendingUserInput) -> Value {
    json!({
        "question": request.question,
        "options": request.options,
        "note": request.note,
    })
}

fn persisted_pending_interactions(request: Option<&PendingUserInput>) -> Vec<PersistedInteraction> {
    request
        .map(|request| {
            vec![PersistedInteraction {
                kind: "request_user_input".to_string(),
                status: "pending".to_string(),
                title: request.question.clone(),
                summary: request.note.clone().unwrap_or_default(),
                payload: Some(serialize_pending_user_input(request)),
            }]
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use async_trait::async_trait;
    use rara_memory::vectordb::VectorDB;
    use rara_state::state_db::{PersistedStructuredRolloutEvent, StateDb};
    use rara_state::thread_rollout_log;
    use rara_tools::tool::{Tool, ToolCallContext, ToolError};
    use serde_json::json;
    use tempfile::tempdir;
    use tokio::time::{Duration, sleep};

    use super::{
        BACKGROUND_SUBAGENT_COMPLETED_RETENTION, BackgroundSubAgentRecord, BackgroundSubAgentStore,
        SubAgentKind, TEAM_CREATE_CONCURRENCY_LIMIT, append_subagent_prompt,
        build_read_only_tool_manager, build_subagent_tool_manager,
        latest_assistant_text_from_history, parse_team_task_kind,
    };
    use crate::agent::Message;
    use crate::llm::{ContentBlock, LlmBackend, LlmResponse, MockLlm};
    use crate::prompt::PromptRuntimeConfig;
    use crate::session::SessionManager;
    use crate::session_transcript::{load_transcript, model_visible_messages};
    use crate::thread_store::{ThreadMetadataSource, ThreadStore};
    use crate::tools::agent::{
        AgentTool, ExploreAgentTool, PlanAgentTool, SubAgentResumeTool, SubAgentStopTool,
        TeamCreateTool,
    };
    use crate::workspace::WorkspaceMemory;

    struct CountingBackend {
        calls: Arc<AtomicUsize>,
    }

    struct PeakBackend {
        in_flight: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
    }

    struct PlanStateBackend;

    struct SlowBackend;

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
    impl LlmBackend for PlanStateBackend {
        async fn ask(
            &self,
            _messages: &[Message],
            _tools: &[serde_json::Value],
        ) -> anyhow::Result<LlmResponse> {
            Ok(LlmResponse {
                content: vec![ContentBlock::Text {
                    text: "<proposed_plan>\n- [pending] Inspect subagent restore\n- [in_progress] Persist child state\n</proposed_plan>\nPlan state ready.".to_string(),
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

    #[async_trait]
    impl LlmBackend for SlowBackend {
        async fn ask(
            &self,
            messages: &[Message],
            _tools: &[serde_json::Value],
        ) -> anyhow::Result<LlmResponse> {
            sleep(Duration::from_millis(250)).await;
            let last = messages
                .last()
                .and_then(|message| message.content.as_str())
                .unwrap_or_default();
            Ok(LlmResponse {
                content: vec![ContentBlock::Text {
                    text: format!("slow {last}"),
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
    async fn team_create_rejects_unstable_explicit_name_before_running_subagents() {
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
                        "name": "!!!",
                        "instruction": "should not run"
                    }
                ]
            }))
            .await
            .expect_err("invalid name");

        assert!(matches!(err, ToolError::InvalidInput(message)
                if message == "tasks[0].name must normalize to a non-empty agent id label"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn spawn_agent_rejects_name_that_normalizes_empty_before_running_subagent() {
        let calls = Arc::new(AtomicUsize::new(0));
        let temp = tempdir().expect("tempdir");
        let root = temp.path().join("workspace");
        let rara_dir = temp.path().join(".rara");
        std::fs::create_dir_all(&root).expect("workspace");
        let tool = AgentTool {
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
            background_subagents: Arc::new(BackgroundSubAgentStore::default()),
        };

        let err = tool
            .call(json!({
                "name": "!!!",
                "instruction": "should not run"
            }))
            .await
            .expect_err("invalid name");

        assert!(matches!(err, ToolError::InvalidInput(message)
                if message == "name must normalize to a non-empty agent id label"));
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
        let result_status = item["status"].as_str().expect("status");
        let result_summary = item["summary"].as_str().expect("summary");
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

        let events =
            thread_rollout_log::load_rollout_events(&rara_dir.join("rollouts"), "parent-session")
                .expect("rollout events");
        assert!(matches!(
            events.as_slice(),
            [PersistedStructuredRolloutEvent::SpawnAgent {
                event_id,
                agent_id: event_agent_id,
                name: Some(name),
                child_session_id: event_child_session_id,
                status,
                summary: Some(summary),
                ..
            }] if event_id.starts_with("spawn-")
                && event_agent_id == agent_id
                && name == "Review Worker"
                && event_child_session_id == child_session_id
                && status == result_status
                && summary == result_summary
        ));
    }

    #[tokio::test]
    async fn spawn_agent_writes_parent_scoped_sidechain_transcript() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path().join("workspace");
        let rara_dir = temp.path().join(".rara");
        std::fs::create_dir_all(&root).expect("workspace");
        let tool = AgentTool {
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
            background_subagents: Arc::new(BackgroundSubAgentStore::default()),
        };

        let mut progress = |_| {};
        let result = tool
            .call_with_context_events(
                json!({
                    "name": "General Worker",
                    "instruction": "summarize this task"
                }),
                ToolCallContext::default().with_session_id("parent-session"),
                &mut progress,
            )
            .await
            .expect("spawn_agent result");

        let agent_id = result["agent_id"].as_str().expect("agent_id");
        let child_session_id = result["session_id"].as_str().expect("session_id");
        assert!(agent_id.starts_with("general-worker-"));
        let transcript_path = rara_dir
            .join("rollouts")
            .join("parent-session")
            .join("subagents")
            .join(format!("agent-{agent_id}.jsonl"));
        let transcript = load_transcript(&transcript_path).expect("transcript");
        assert_eq!(transcript.parse_errors, 0);
        assert!(model_visible_messages(&transcript.entries).is_empty());

        let events =
            thread_rollout_log::load_rollout_events(&rara_dir.join("rollouts"), "parent-session")
                .expect("rollout events");
        assert!(matches!(
            events.as_slice(),
            [PersistedStructuredRolloutEvent::SpawnAgent {
                agent_id: event_agent_id,
                name: Some(name),
                child_session_id: event_child_session_id,
                status,
                ..
            }] if event_agent_id == agent_id
                && name == "General Worker"
                && event_child_session_id == child_session_id
                && status == "done"
        ));

        let state_db = StateDb::new_for_root_dir(rara_dir).expect("state db");
        let thread_store = ThreadStore::new(tool.session_manager.as_ref(), &state_db);
        let child = thread_store
            .load_thread(child_session_id)
            .expect("child thread");
        assert_eq!(
            child.provenance.metadata_source,
            ThreadMetadataSource::StructuredMetadata
        );
        assert_eq!(child.metadata.origin_kind, "subagent");
        assert_eq!(
            child.metadata.forked_from_thread_id.as_deref(),
            Some("parent-session")
        );
        assert!(!child.history.is_empty());
    }

    #[tokio::test]
    async fn background_subagent_resume_returns_completed_summary_without_inline_sidechain() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path().join("workspace");
        let rara_dir = temp.path().join(".rara");
        std::fs::create_dir_all(&root).expect("workspace");
        let background_subagents = Arc::new(BackgroundSubAgentStore::default());
        let session_manager =
            Arc::new(SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"));
        let tool = ExploreAgentTool {
            backend: Arc::new(CountingBackend {
                calls: Arc::new(AtomicUsize::new(0)),
            }),
            vdb: Arc::new(VectorDB::new(
                &rara_dir.join("lancedb").display().to_string(),
            )),
            session_manager,
            workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir.clone())),
            prompt_config: PromptRuntimeConfig::default(),
            background_subagents: background_subagents.clone(),
        };
        let resume = SubAgentResumeTool {
            background_subagents,
        };

        let mut progress = |_| {};
        let started = tool
            .call_with_context_events(
                json!({
                    "instruction": "inspect this in the background",
                    "run_in_background": true
                }),
                ToolCallContext::default().with_session_id("parent-session"),
                &mut progress,
            )
            .await
            .expect("background start");
        let agent_id = started["agent_id"].as_str().expect("agent_id");
        let child_session_id = started["session_id"].as_str().expect("session_id");
        assert_eq!(started["status"], "running");

        let mut completed = None;
        for _ in 0..20 {
            let status = resume
                .call(json!({ "agent_id": agent_id }))
                .await
                .expect("resume status");
            if status["status"] != "running" {
                completed = Some(status);
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
        let completed = completed.expect("background sub-agent completed");
        assert_eq!(completed["status"], "explored");
        assert!(
            completed["summary"]
                .as_str()
                .expect("summary")
                .starts_with("counted")
        );
        assert_eq!(completed["session_id"], child_session_id);

        let transcript_path = rara_dir
            .join("rollouts")
            .join("parent-session")
            .join("subagents")
            .join(format!("agent-{agent_id}.jsonl"));
        let transcript = load_transcript(&transcript_path).expect("transcript");
        assert_eq!(transcript.parse_errors, 0);
        assert!(model_visible_messages(&transcript.entries).is_empty());
    }

    #[tokio::test]
    async fn background_subagent_stop_marks_running_task_cancelled() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path().join("workspace");
        let rara_dir = temp.path().join(".rara");
        std::fs::create_dir_all(&root).expect("workspace");
        let background_subagents = Arc::new(BackgroundSubAgentStore::default());
        let tool = ExploreAgentTool {
            backend: Arc::new(SlowBackend),
            vdb: Arc::new(VectorDB::new(
                &rara_dir.join("lancedb").display().to_string(),
            )),
            session_manager: Arc::new(
                SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
            ),
            workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir)),
            prompt_config: PromptRuntimeConfig::default(),
            background_subagents: background_subagents.clone(),
        };
        let stop = SubAgentStopTool {
            background_subagents: background_subagents.clone(),
        };
        let resume = SubAgentResumeTool {
            background_subagents,
        };

        let mut progress = |_| {};
        let started = tool
            .call_with_context_events(
                json!({
                    "instruction": "keep running until stopped",
                    "run_in_background": true
                }),
                ToolCallContext::default().with_session_id("parent-session"),
                &mut progress,
            )
            .await
            .expect("background start");
        let agent_id = started["agent_id"].as_str().expect("agent_id");

        let stopped = stop
            .call(json!({ "agent_id": agent_id }))
            .await
            .expect("stop sub-agent");
        assert_eq!(stopped["status"], "cancelled");

        let resumed = resume
            .call(json!({ "agent_id": agent_id }))
            .await
            .expect("resume cancelled sub-agent");
        assert_eq!(resumed["status"], "cancelled");
        assert!(resumed["finished_at"].as_u64().is_some());
    }

    #[test]
    fn background_subagent_store_prunes_old_completed_records() {
        let store = BackgroundSubAgentStore::default();
        {
            let mut inner = store.inner.lock().expect("store");
            for idx in 0..(BACKGROUND_SUBAGENT_COMPLETED_RETENTION + 3) {
                let agent_id = format!("agent-{idx}");
                inner.tasks.insert(
                    agent_id.clone(),
                    BackgroundSubAgentRecord {
                        agent_id,
                        session_id: format!("session-{idx}"),
                        name: None,
                        kind: "general",
                        parent_session_id: None,
                        status: "done",
                        summary: Some(format!("summary {idx}")),
                        error: None,
                        persistence_error: None,
                        plan: None,
                        plan_explanation: None,
                        request_user_input: None,
                        started_at: idx as u64,
                        finished_at: Some(idx as u64),
                    },
                );
            }
        }

        store.finish(
            &format!("agent-{}", BACKGROUND_SUBAGENT_COMPLETED_RETENTION + 2),
            Err(ToolError::ExecutionFailed("refresh latest".to_string())),
        );

        let records = store.list().expect("records");
        assert_eq!(records.len(), BACKGROUND_SUBAGENT_COMPLETED_RETENTION);
        assert!(records.iter().any(|record| record.agent_id == "agent-66"));
        assert!(!records.iter().any(|record| record.agent_id == "agent-0"));
    }

    #[tokio::test]
    async fn background_plan_agent_resume_returns_plan_state() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path().join("workspace");
        let rara_dir = temp.path().join(".rara");
        std::fs::create_dir_all(&root).expect("workspace");
        let background_subagents = Arc::new(BackgroundSubAgentStore::default());
        let tool = PlanAgentTool {
            backend: Arc::new(PlanStateBackend),
            vdb: Arc::new(VectorDB::new(
                &rara_dir.join("lancedb").display().to_string(),
            )),
            session_manager: Arc::new(
                SessionManager::new_for_rara_dir(rara_dir).expect("session manager"),
            ),
            workspace: Arc::new(WorkspaceMemory::from_paths(root, temp.path().join(".rara"))),
            prompt_config: PromptRuntimeConfig::default(),
            background_subagents: background_subagents.clone(),
        };
        let resume = SubAgentResumeTool {
            background_subagents,
        };

        let mut progress = |_| {};
        let started = tool
            .call_with_context_events(
                json!({
                    "instruction": "plan this in the background",
                    "run_in_background": true
                }),
                ToolCallContext::default().with_session_id("parent-session"),
                &mut progress,
            )
            .await
            .expect("background start");
        let agent_id = started["agent_id"].as_str().expect("agent_id");

        let mut completed = None;
        for _ in 0..20 {
            let status = resume
                .call(json!({ "agent_id": agent_id }))
                .await
                .expect("resume status");
            if status["status"] != "running" {
                completed = Some(status);
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
        let completed = completed.expect("background plan sub-agent completed");
        assert_eq!(completed["status"], "planned");
        let steps = completed["plan"].as_array().expect("plan steps");
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0]["step"], "Inspect subagent restore");
        assert_eq!(steps[1]["status"], "in_progress");
    }

    #[tokio::test]
    async fn plan_agent_writes_parent_scoped_sidechain_transcript() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path().join("workspace");
        let rara_dir = temp.path().join(".rara");
        std::fs::create_dir_all(&root).expect("workspace");
        let tool = PlanAgentTool {
            backend: Arc::new(PlanStateBackend),
            vdb: Arc::new(VectorDB::new(
                &rara_dir.join("lancedb").display().to_string(),
            )),
            session_manager: Arc::new(
                SessionManager::new_for_rara_dir(rara_dir.clone()).expect("session manager"),
            ),
            workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir.clone())),
            prompt_config: PromptRuntimeConfig::default(),
            background_subagents: Arc::new(BackgroundSubAgentStore::default()),
        };

        let mut progress = |_| {};
        let result = tool
            .call_with_context_events(
                json!({ "instruction": "plan this task" }),
                ToolCallContext::default().with_session_id("parent-session"),
                &mut progress,
            )
            .await
            .expect("plan_agent result");

        let agent_id = result["agent_id"].as_str().expect("agent_id");
        let child_session_id = result["session_id"].as_str().expect("session_id");
        assert!(agent_id.starts_with("plan-"));
        let transcript_path = rara_dir
            .join("rollouts")
            .join("parent-session")
            .join("subagents")
            .join(format!("agent-{agent_id}.jsonl"));
        let transcript = load_transcript(&transcript_path).expect("transcript");
        assert_eq!(transcript.parse_errors, 0);
        assert!(model_visible_messages(&transcript.entries).is_empty());

        let events =
            thread_rollout_log::load_rollout_events(&rara_dir.join("rollouts"), "parent-session")
                .expect("rollout events");
        assert!(matches!(
            events.as_slice(),
            [PersistedStructuredRolloutEvent::SpawnAgent {
                agent_id: event_agent_id,
                name: None,
                child_session_id: event_child_session_id,
                status,
                ..
            }] if event_agent_id == agent_id
                && event_child_session_id == child_session_id
                && status == "planned"
        ));

        let state_db = StateDb::new_for_root_dir(rara_dir).expect("state db");
        let thread_store = ThreadStore::new(tool.session_manager.as_ref(), &state_db);
        let child = thread_store
            .load_thread(child_session_id)
            .expect("child thread");
        assert_eq!(
            child.provenance.metadata_source,
            ThreadMetadataSource::StructuredMetadata
        );
        assert_eq!(child.plan_steps.len(), 2);
        assert_eq!(child.plan_steps[0].step, "Inspect subagent restore");
        assert_eq!(child.plan_steps[0].status, "pending");
        assert_eq!(child.plan_steps[1].step, "Persist child state");
        assert_eq!(child.plan_steps[1].status, "in_progress");
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
            background_subagents: Arc::new(BackgroundSubAgentStore::default()),
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
        assert!(
            thread_rollout_log::load_rollout_events(&rara_dir.join("rollouts"), "parent-session")
                .expect("rollout events")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn subagent_returns_result_when_sidechain_persistence_fails() {
        let temp = tempdir().expect("tempdir");
        let root = temp.path().join("workspace");
        let rara_dir = temp.path().join(".rara");
        std::fs::create_dir_all(&root).expect("workspace");
        let rollouts_dir = rara_dir.join("rollouts");
        std::fs::create_dir_all(&rollouts_dir).expect("rollouts");
        std::fs::write(rollouts_dir.join("blocked-parent"), b"not a directory")
            .expect("blocking file");
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
            workspace: Arc::new(WorkspaceMemory::from_paths(root, rara_dir)),
            prompt_config: PromptRuntimeConfig::default(),
            background_subagents: Arc::new(BackgroundSubAgentStore::default()),
        };

        let mut progress = |_| {};
        let result = tool
            .call_with_context_events(
                json!({ "instruction": "look around" }),
                ToolCallContext::default().with_session_id("blocked-parent"),
                &mut progress,
            )
            .await
            .expect("explore result");

        assert_eq!(result["status"], "explored");
        assert!(
            result["summary"]
                .as_str()
                .expect("summary")
                .starts_with("counted")
        );
        assert!(
            result["persistence_error"]
                .as_str()
                .expect("persistence error")
                .len()
                > 0
        );
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
