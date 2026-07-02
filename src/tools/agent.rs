// Agent tool items reserved for subagent and team features.
// NOTE: dead_code retained — context types shared across sub-agents.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use futures::stream::{self, StreamExt, TryStreamExt};
use rara_memory::vectordb::VectorDB;
use rara_persistence::thread_data::{
    PersistedCompactState, PersistedInteraction, PersistedPlanStep, PersistedPromptRuntimeState,
};
use rara_state::state_db::StateDb;
use rara_tool_macros::tool_spec;
use rara_tools::file::{ListFilesTool, ReadFileTool};
use rara_tools::search::{GlobTool, GrepTool};
use rara_tools::tool::{Tool, ToolCallContext, ToolError, ToolManager};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::agent::{
    Agent, AgentExecutionMode, Message, PendingUserInput, PlanStep, PlanStepStatus,
};
use crate::llm::{EmbeddingBackend, LlmBackend};
use crate::prompt::PromptRuntimeConfig;
use crate::session::SessionManager;
use crate::session_transcript::{self, TranscriptScope};
use crate::tasklist::TaskListStore;
use crate::thread_store::{ThreadRecorder, ThreadRuntimeLineage, ThreadRuntimeState};
use crate::tools::tasklist::{TaskCreateTool, TaskGetTool, TaskListTool, TaskUpdateTool};
use crate::workspace::WorkspaceMemory;

#[derive(Clone, Copy, Debug)]
pub(crate) enum SubAgentKind {
    General,
    Explore,
    Plan,
    Consolidate,
}

/// Maps Claude Code agent config tool names to RARA internal tool names.
/// Maps Claude Code config tool names (agent yaml) to RARA internal tool names.
pub(super) fn agent_tool_to_internal_name(name: &str) -> &str {
    match name {
        "Bash" => "bash",
        "Read" => "read_file",
        "Write" => "write_file",
        "Edit" => "apply_patch",
        "Glob" => "glob",
        "Grep" => "grep",
        "WebSearch" => "web_search",
        "WebFetch" => "web_fetch",
        "TaskCreate" => "task_create",
        "TaskList" => "task_list",
        "TaskGet" => "task_get",
        "TaskUpdate" => "task_update",
        "Task" | "spawn_agent" => "spawn_agent",
        n => n,
    }
}

// Agent definition matching RARA .rara/agents/*.md files with Claude-compatible frontmatter.
include!("agent_def.rs");

/// Progress tracking for a subagent (Claude Code AgentProgress compatible).
#[derive(Clone, Debug)]
pub struct SubagentProgress {
    pub tool_use_count: usize,
    pub tool_use_total: Option<usize>,
    pub total_input_tokens: usize,
    pub total_output_tokens: usize,
    pub total_cache_hit_tokens: usize,
    pub total_cache_miss_tokens: usize,
    pub activity: Vec<String>,
    /// Reserved for background subagent UI state.
    /// Will be activated with queued background subagent messages
    /// (docs/features/subagent-and-aux-compression.md).
    #[allow(dead_code)]
    pub is_backgrounded: bool,
    /// Reserved for background subagent progress labels.
    /// Will be activated with queued background subagent messages
    /// (docs/features/subagent-and-aux-compression.md).
    #[allow(dead_code)]
    pub subagent_name: String,
}

impl SubagentProgress {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            tool_use_count: 0,
            tool_use_total: None,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cache_hit_tokens: 0,
            total_cache_miss_tokens: 0,
            activity: Vec::new(),
            is_backgrounded: false,
            subagent_name: name.into(),
        }
    }

    /// Reserved for per-tool subagent activity summaries.
    /// Will be activated with subagent progress tracking
    /// (docs/features/subagent-and-aux-compression.md).
    #[allow(dead_code)]
    pub fn record_activity(&mut self, desc: impl Into<String>) {
        let s = desc.into();
        self.activity.push(s);
        if self.activity.len() > 5 {
            self.activity.remove(0);
        }
    }

    pub fn latest_activity(&self) -> Option<&str> {
        self.activity.last().map(|s| s.as_str())
    }

    pub fn total_tokens(&self) -> usize {
        self.total_input_tokens + self.total_output_tokens
    }
}

const TEAM_CREATE_MAX_TASKS: usize = 8;
const TEAM_CREATE_CONCURRENCY_LIMIT: usize = 4;
const SUBAGENT_TIMEOUT_SECS: u64 = 300;

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
            "- Use only the read-only repository inspection tools available to you: read_file, list_files, glob, grep, task_list, task_get.\n",
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
            SubAgentKind::Consolidate => "completed consolidation",
        }
    }

    fn append_prompt(self) -> &'static str {
        match self {
            SubAgentKind::General => {
                concat!(
                    "## Sub-Agent Role\n",
                    "- You are a shared-task worker sub-agent.\n",
                    "- Treat the assigned instruction as the complete task contract.\n",
                    "- Honor every constraint in the assigned instruction, including workspace, branch, network, and output limits.\n",
                    "- Stay inside the current workspace unless the assigned instruction explicitly allows another path.\n",
                    "- You do not have repository, shell, editing, patching, or browser tools in this role.\n",
                    "- You may use shared task-list tools to inspect, claim, update, or complete project tasks.\n",
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
            SubAgentKind::Consolidate => {
                concat!(
                    "## Sub-Agent Role\n",
                    "- You are a memory consolidation agent.\n",
                    "- Read the listed session files and distill durable knowledge.\n",
                    "- Write topic files under `topics/` and update the `MEMORY.md` index.\n",
                    "- MEMORY.md is an index — one line per topic under 150 characters.\n",
                    "- Do not write memory content directly into MEMORY.md.\n",
                    "- Group facts by topic, write concise topic files with level-2 headings.\n",
                    "- Prefer updating existing files over creating duplicates.\n",
                    "- Do not delegate to another agent or spawn sub-agents.\n",
                    "- IMPORTANT: only read/write files inside the memory directory.\n",
                    "  Never modify files outside the memory directory —\n",
                    "  MEMORY.md, topics/, and sessions/ are all under it.\n",
                    "  Session files under sessions/ are read-only inputs.\n",
                    "- Report a brief summary of what you changed."
                )
            }
        }
    }

    fn execution_mode(self) -> AgentExecutionMode {
        match self {
            SubAgentKind::Plan => AgentExecutionMode::Plan,
            SubAgentKind::General | SubAgentKind::Explore | SubAgentKind::Consolidate => {
                AgentExecutionMode::Execute
            }
        }
    }

    fn default_max_turns(self) -> usize {
        match self {
            SubAgentKind::Plan => 200,
            SubAgentKind::General => 100,
            SubAgentKind::Explore => 100,
            SubAgentKind::Consolidate => 200,
        }
    }

    fn read_only(self) -> bool {
        matches!(self, SubAgentKind::Explore | SubAgentKind::Plan)
    }

    fn label(self) -> &'static str {
        match self {
            SubAgentKind::General => "general",
            SubAgentKind::Explore => "explore",
            SubAgentKind::Plan => "plan",
            SubAgentKind::Consolidate => "consolidate",
        }
    }
}

pub struct AgentTool {
    pub backend: Arc<dyn LlmBackend>,
    pub embedding_backend: Arc<dyn EmbeddingBackend>,
    pub vdb: Arc<VectorDB>,
    pub session_manager: Arc<SessionManager>,
    pub workspace: Arc<WorkspaceMemory>,
    pub prompt_config: PromptRuntimeConfig,
    pub background_subagents: Arc<BackgroundSubAgentStore>,
    pub task_list_id: String,
}

#[tool_spec(
    name = "spawn_agent",
    description = "Spawn a shared-task worker sub-agent. It cannot inspect files, run shell commands, edit files, or spawn other agents; it can inspect and update shared task-list entries. Use explore_agent or plan_agent for read-only repository inspection.",
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
        let Some(agent_label) = validate_agent_id_label(name) else {
            return Err(ToolError::InvalidInput(
                "name must normalize to a non-empty agent id label".into(),
            ));
        };
        let instruction = i["instruction"]
            .as_str()
            .ok_or(ToolError::InvalidInput("instruction".into()))?;
        let agent_id = next_subagent_id(SubAgentKind::General, Some(name));
        let definition = resolve_spawn_agent_definition(&self.workspace.root, &agent_label);
        if i.get("run_in_background")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let task = self.background_subagents.start(BackgroundSubAgentStart {
                kind: SubAgentKind::General,
                agent_id,
                name: Some(name.to_string()),
                definition,
                parent_session_id: parent_session_id.map(str::to_string),
                instruction: instruction.to_string(),
                backend: self.backend.clone(),
                embedding_backend: self.embedding_backend.clone(),
                vdb: self.vdb.clone(),
                session_manager: self.session_manager.clone(),
                workspace: self.workspace.clone(),
                prompt_config: self.prompt_config.clone(),
                task_list_id: self.task_list_id.clone(),
            })?;
            return Ok(task.to_json());
        }
        let result = run_sub_agent(
            SubAgentKind::General,
            &agent_id,
            Some(&definition),
            Some(name),
            parent_session_id,
            instruction,
            None,
            None,
            self.backend.clone(),
            self.embedding_backend.clone(),
            self.vdb.clone(),
            self.session_manager.clone(),
            self.workspace.clone(),
            self.prompt_config.clone(),
            self.task_list_id.clone(),
        )
        .await?;
        Ok(json!({
            "agent_id": result.agent_id,
            "session_id": result.session_id,
            "name": name,
            "status": result.status,
            "summary": result.summary,
            "cache_hit_tokens": result.total_cache_hit_tokens,
            "cache_miss_tokens": result.total_cache_miss_tokens,
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
    pub embedding_backend: Arc<dyn EmbeddingBackend>,
    pub vdb: Arc<VectorDB>,
    pub session_manager: Arc<SessionManager>,
    pub workspace: Arc<WorkspaceMemory>,
    pub prompt_config: PromptRuntimeConfig,
    pub background_subagents: Arc<BackgroundSubAgentStore>,
    pub task_list_id: String,
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
                definition: resolve_kind_definition(SubAgentKind::Explore),
                parent_session_id: parent_session_id.map(str::to_string),
                instruction: instruction.to_string(),
                backend: self.backend.clone(),
                embedding_backend: self.embedding_backend.clone(),
                vdb: self.vdb.clone(),
                session_manager: self.session_manager.clone(),
                workspace: self.workspace.clone(),
                prompt_config: self.prompt_config.clone(),
                task_list_id: self.task_list_id.clone(),
            })?;
            return Ok(task.to_json());
        }
        let result = run_sub_agent(
            SubAgentKind::Explore,
            &agent_id,
            None,
            None,
            parent_session_id,
            instruction,
            None,
            None,
            self.backend.clone(),
            self.embedding_backend.clone(),
            self.vdb.clone(),
            self.session_manager.clone(),
            self.workspace.clone(),
            self.prompt_config.clone(),
            self.task_list_id.clone(),
        )
        .await?;
        Ok(json!({
            "agent_id": result.agent_id,
            "session_id": result.session_id,
            "status": result.status,
            "summary": result.summary,
            "cache_hit_tokens": result.total_cache_hit_tokens,
            "cache_miss_tokens": result.total_cache_miss_tokens,
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
    pub embedding_backend: Arc<dyn EmbeddingBackend>,
    pub vdb: Arc<VectorDB>,
    pub session_manager: Arc<SessionManager>,
    pub workspace: Arc<WorkspaceMemory>,
    pub prompt_config: PromptRuntimeConfig,
    pub background_subagents: Arc<BackgroundSubAgentStore>,
    pub task_list_id: String,
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
                definition: resolve_kind_definition(SubAgentKind::Plan),
                parent_session_id: parent_session_id.map(str::to_string),
                instruction: instruction.to_string(),
                backend: self.backend.clone(),
                embedding_backend: self.embedding_backend.clone(),
                vdb: self.vdb.clone(),
                session_manager: self.session_manager.clone(),
                workspace: self.workspace.clone(),
                prompt_config: self.prompt_config.clone(),
                task_list_id: self.task_list_id.clone(),
            })?;
            return Ok(task.to_json());
        }
        let result = run_sub_agent(
            SubAgentKind::Plan,
            &agent_id,
            None,
            None,
            parent_session_id,
            instruction,
            None,
            None,
            self.backend.clone(),
            self.embedding_backend.clone(),
            self.vdb.clone(),
            self.session_manager.clone(),
            self.workspace.clone(),
            self.prompt_config.clone(),
            self.task_list_id.clone(),
        )
        .await?;
        Ok(json!({
            "agent_id": result.agent_id,
            "session_id": result.session_id,
            "status": result.status,
            "summary": result.summary,
            "cache_hit_tokens": result.total_cache_hit_tokens,
            "cache_miss_tokens": result.total_cache_miss_tokens,
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
    pub embedding_backend: Arc<dyn EmbeddingBackend>,
    pub vdb: Arc<VectorDB>,
    pub session_manager: Arc<SessionManager>,
    pub workspace: Arc<WorkspaceMemory>,
    pub prompt_config: PromptRuntimeConfig,
    pub task_list_id: String,
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
    definition: AgentDefinition,
    parent_session_id: Option<String>,
    instruction: String,
    backend: Arc<dyn LlmBackend>,
    embedding_backend: Arc<dyn EmbeddingBackend>,
    vdb: Arc<VectorDB>,
    session_manager: Arc<SessionManager>,
    workspace: Arc<WorkspaceMemory>,
    prompt_config: PromptRuntimeConfig,
    task_list_id: String,
}

#[derive(Clone, Debug)]
struct BackgroundSubAgentRecord {
    agent_id: String,
    session_id: String,
    name: Option<String>,
    model: Option<String>,
    progress: SubagentProgress,
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
            model: start.definition.model.clone(),
            progress: SubagentProgress::new(
                start.name.clone().unwrap_or_else(|| "sub-agent".into()),
            ),
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
                Some(&start.definition),
                start.name.as_deref(),
                start.parent_session_id.as_deref(),
                &start.instruction,
                Some(session_id),
                Some(cancellation),
                start.backend,
                start.embedding_backend,
                start.vdb,
                start.session_manager,
                start.workspace,
                start.prompt_config,
                start.task_list_id,
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
                record.progress.total_input_tokens = result.total_input_tokens as usize;
                record.progress.total_output_tokens = result.total_output_tokens as usize;
                record.progress.total_cache_hit_tokens = result.total_cache_hit_tokens as usize;
                record.progress.total_cache_miss_tokens = result.total_cache_miss_tokens as usize;
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
            "model": self.model,
            "progress": {
                "tool_use_count": self.progress.tool_use_count,
                "tool_use_total": self.progress.tool_use_total,
                "latest_activity": self.progress.latest_activity(),
                "total_input_tokens": self.progress.total_input_tokens,
                "total_output_tokens": self.progress.total_output_tokens,
                "total_tokens": self.progress.total_tokens(),
            },
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
            let embedding_backend = self.embedding_backend.clone();
            let vdb = self.vdb.clone();
            let session_manager = self.session_manager.clone();
            let workspace = self.workspace.clone();
            let prompt_config = self.prompt_config.clone();
            let task_list_id = self.task_list_id.clone();
            let parent_session_id = parent_session_id.map(str::to_string);
            let agent_id = next_subagent_id(task.kind, Some(&task.name));

            async move {
                let result = run_sub_agent(
                    task.kind,
                    &agent_id,
                    None,
                    Some(&task.name),
                    parent_session_id.as_deref(),
                    &task.instruction,
                    None,
                    None,
                    backend,
                    embedding_backend,
                    vdb,
                    session_manager,
                    workspace,
                    prompt_config,
                    task_list_id,
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

pub(crate) struct SubAgentResult {
    agent_id: String,
    session_id: String,
    pub(crate) status: &'static str,
    pub(crate) summary: String,
    persistence_error: Option<String>,
    plan: Option<Vec<PlanStep>>,
    plan_explanation: Option<String>,
    request_user_input: Option<PendingUserInput>,
    pub(crate) total_input_tokens: u32,
    pub(crate) total_output_tokens: u32,
    pub(crate) total_cache_hit_tokens: u32,
    pub(crate) total_cache_miss_tokens: u32,
}

#[allow(clippy::too_many_arguments)]
// Sub-agent execution is called from multiple tool entrypoints with explicit
// runtime handles; grouping them would obscure the execution boundary.
pub(crate) async fn run_sub_agent(
    kind: SubAgentKind,
    agent_id: &str,
    definition: Option<&AgentDefinition>,
    name: Option<&str>,
    parent_session_id: Option<&str>,
    instruction: &str,
    session_id: Option<String>,
    cancellation_token: Option<Arc<AtomicBool>>,
    backend: Arc<dyn LlmBackend>,
    embedding_backend: Arc<dyn EmbeddingBackend>,
    vdb: Arc<VectorDB>,
    session_manager: Arc<SessionManager>,
    workspace: Arc<WorkspaceMemory>,
    prompt_config: PromptRuntimeConfig,
    task_list_id: String,
) -> Result<SubAgentResult, ToolError> {
    let tool_manager = if let Some(def) = definition {
        build_filtered_tool_manager(kind, def, workspace.rara_dir.join("tasks"), &task_list_id)
    } else {
        build_subagent_tool_manager(kind, workspace.rara_dir.join("tasks"), &task_list_id)
    };
    let mut sub = Agent::new_with_embedding_backend(
        tool_manager,
        backend,
        embedding_backend,
        vdb,
        session_manager.clone(),
        workspace.clone(),
    );
    if let Some(session_id) = session_id {
        sub.session_id = session_id;
    }
    sub.set_cancellation_token(cancellation_token);
    sub.set_execution_mode(if definition.is_some_and(|d| d.plan_mode_required) {
        AgentExecutionMode::Plan
    } else {
        kind.execution_mode()
    });
    let appended_prompt = match definition
        .map(|d| d.system_prompt.trim())
        .filter(|prompt| !prompt.is_empty())
    {
        Some(system_prompt) => format!("{}\n\n{}", kind.append_prompt(), system_prompt),
        None => kind.append_prompt().to_string(),
    };
    sub.set_prompt_config(append_subagent_prompt(prompt_config, &appended_prompt));
    sub.task_list_id = task_list_id;

    let def_max_turns = definition
        .and_then(|d| {
            if d.max_turns > 0 {
                Some(d.max_turns)
            } else {
                None
            }
        })
        .unwrap_or_else(|| kind.default_max_turns());
    sub.set_max_turns(def_max_turns);

    let query_fut = sub.query_with_mode(
        instruction.to_string(),
        crate::agent::AgentOutputMode::Silent,
    );

    tokio::time::timeout(Duration::from_secs(SUBAGENT_TIMEOUT_SECS), query_fut)
        .await
        .map_err(|_elapsed| {
            ToolError::ExecutionFailed(format!(
                "sub-agent {} ({}) timed out after {} seconds",
                agent_id,
                kind.label(),
                SUBAGENT_TIMEOUT_SECS
            ))
        })?
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
        total_input_tokens: sub.total_input_tokens,
        total_output_tokens: sub.total_output_tokens,
        total_cache_hit_tokens: sub.total_cache_hit_tokens,
        total_cache_miss_tokens: sub.total_cache_miss_tokens,
        status,
        summary,
        persistence_error,
        plan: (!sub.current_plan.is_empty()).then_some(sub.current_plan.clone()),
        plan_explanation: sub.plan_explanation.clone(),
        request_user_input: sub.pending_user_input.clone(),
    })
}

#[allow(clippy::too_many_arguments)]
// Persistence mirrors the spawn-agent rollout edge fields.
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

fn build_read_only_tool_manager(
    task_store: Arc<TaskListStore>,
    default_task_list_id: &str,
) -> ToolManager {
    // Keep this registration set synchronized with strict_read_only_subagent_prompt!().
    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(ReadFileTool::default()));
    tool_manager.register(Box::new(ListFilesTool));
    tool_manager.register(Box::new(GlobTool));
    tool_manager.register(Box::new(GrepTool));
    tool_manager.register(Box::new(TaskListTool {
        store: task_store.clone(),
        default_task_list_id: default_task_list_id.to_string(),
    }));
    tool_manager.register(Box::new(TaskGetTool {
        store: task_store,
        default_task_list_id: default_task_list_id.to_string(),
    }));
    tool_manager
}

pub(crate) fn build_subagent_tool_manager(
    kind: SubAgentKind,
    task_root: PathBuf,
    default_task_list_id: &str,
) -> ToolManager {
    let task_store = Arc::new(TaskListStore::new(task_root));
    if kind.read_only() {
        build_read_only_tool_manager(task_store, default_task_list_id)
    } else {
        let mut tool_manager = ToolManager::new();
        tool_manager.register(Box::new(TaskCreateTool {
            store: task_store.clone(),
            default_task_list_id: default_task_list_id.to_string(),
        }));
        tool_manager.register(Box::new(TaskListTool {
            store: task_store.clone(),
            default_task_list_id: default_task_list_id.to_string(),
        }));
        tool_manager.register(Box::new(TaskUpdateTool {
            store: task_store.clone(),
            default_task_list_id: default_task_list_id.to_string(),
        }));
        tool_manager.register(Box::new(TaskGetTool {
            store: task_store,
            default_task_list_id: default_task_list_id.to_string(),
        }));
        tool_manager
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
        "cache_hit_tokens": result.total_cache_hit_tokens,
        "cache_miss_tokens": result.total_cache_miss_tokens,
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

pub(crate) fn append_subagent_prompt(
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

fn resolve_kind_definition(kind: SubAgentKind) -> AgentDefinition {
    builtin_agent_definition(kind.label()).unwrap_or(AgentDefinition {
        token_budget: None,
        name: kind.label().to_string(),
        description: kind.label().to_string(),
        model: None,
        tools: vec![],
        disallowed_tools: vec![],
        max_turns: 0,
        plan_mode_required: matches!(kind, SubAgentKind::Plan),
        permission_mode: None,
        hidden: false,
        system_prompt: String::new(),
    })
}

fn resolve_spawn_agent_definition(workspace_root: &Path, normalized_name: &str) -> AgentDefinition {
    let registry = load_agent_definitions(workspace_root);
    resolve_agent(normalized_name, &registry)
        .unwrap_or_else(|| fallback_spawn_agent_definition(normalized_name))
}

fn fallback_spawn_agent_definition(name: &str) -> AgentDefinition {
    AgentDefinition {
        token_budget: None,
        name: name.to_string(),
        description: name.to_string(),
        model: None,
        tools: vec![],
        disallowed_tools: vec![],
        max_turns: 0,
        plan_mode_required: false,
        permission_mode: None,
        hidden: false,
        system_prompt: String::new(),
    }
}

fn build_filtered_tool_manager(
    kind: SubAgentKind,
    definition: &AgentDefinition,
    task_root: PathBuf,
    default_task_list_id: &str,
) -> ToolManager {
    let mut tm = build_subagent_tool_manager(kind, task_root, default_task_list_id);

    if !definition.tools.is_empty() {
        let allowed: std::collections::HashSet<&str> = definition
            .tools
            .iter()
            .map(|s| agent_tool_to_internal_name(s))
            .collect();
        tm.retain(|name| allowed.contains(name));
    }
    if !definition.disallowed_tools.is_empty() {
        let blocked: std::collections::HashSet<&str> = definition
            .disallowed_tools
            .iter()
            .map(|s| agent_tool_to_internal_name(s))
            .collect();
        tm.retain(|name| !blocked.contains(name));
    }

    tm
}

#[cfg(test)]
#[path = "agent_test.rs"]
mod tests;
