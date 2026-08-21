// Agent tool items reserved for subagent and team features.
// NOTE: dead_code retained — context types shared across sub-agents.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, RwLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use futures::stream::{self, StreamExt, TryStreamExt};
use rara_memory::memory_handle::MemoryHandle;
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
use crate::llm::LlmBackend;
use crate::prompt::PromptRuntimeConfig;
use crate::session::SessionManager;
use crate::session_transcript::{self, TranscriptScope};
use crate::skill::{SkillManager, SkillScope};
use crate::tasklist::TaskListStore;
use crate::thread_store::{ThreadRecorder, ThreadRuntimeLineage, ThreadRuntimeState};
use crate::tools::skill::{SkillReloadPolicy, SkillTool};
use crate::tools::tasklist::{TaskCreateTool, TaskGetTool, TaskListTool, TaskUpdateTool};
use crate::tools::web::{WebFetchTool, WebSearchTool};
use crate::workspace::WorkspaceMemory;

#[path = "agent_budget.rs"]
mod agent_budget;
#[path = "agent_permission.rs"]
mod agent_permission;
#[path = "agent_reconnect.rs"]
mod agent_reconnect;
#[path = "agent_runtime.rs"]
mod agent_runtime;
use agent_budget::{agent_token_budget, parse_agent_token_budget};
#[path = "agent_control.rs"]
mod agent_control;
#[path = "agent_control_tools.rs"]
mod agent_control_tools;
pub use agent_control::{
    AgentMailboxMessage, AgentSnapshot, AgentTreeConfig, AgentTreeControl, AgentWaitResult,
    BackgroundSubAgentStore,
};
use agent_control::{
    AgentResultDelivery, BACKGROUND_SUBAGENT_COMPLETED_RETENTION, BackgroundSubAgentRecord,
    BackgroundSubAgentStart,
};
pub use agent_control_tools::{
    FollowupTaskTool, InterruptAgentTool, ListAgentsTool, SendAgentMessageTool, SubAgentListTool,
    SubAgentResumeTool, SubAgentStopTool, WaitAgentTool,
};
use agent_permission::{agent_permission_mode, parse_agent_permission_mode};
pub(crate) use agent_runtime::run_sub_agent;
use agent_runtime::*;

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
        "Skill" => "skill",
        "Task" | "spawn_agent" => "spawn_agent",
        n => n,
    }
}

// Agent definition matching RARA .rara/agents/*.md files with Claude-compatible frontmatter.
include!("agent_def.rs");

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SubagentProviderTarget {
    pub provider: Option<String>,
    pub model: Option<String>,
}

#[derive(Clone)]
pub(crate) struct ResolvedSubagentBackend {
    pub backend: Arc<dyn LlmBackend>,
    pub provider: String,
    pub model: String,
}

#[async_trait]
pub(crate) trait SubagentBackendResolver: Send + Sync {
    async fn resolve_backend(
        &self,
        target: Option<&SubagentProviderTarget>,
        inherited_backend: Arc<dyn LlmBackend>,
    ) -> Result<ResolvedSubagentBackend, ToolError>;
}

pub(crate) struct InheritedSubagentBackendResolver;

#[async_trait]
impl SubagentBackendResolver for InheritedSubagentBackendResolver {
    async fn resolve_backend(
        &self,
        target: Option<&SubagentProviderTarget>,
        inherited_backend: Arc<dyn LlmBackend>,
    ) -> Result<ResolvedSubagentBackend, ToolError> {
        let provider = target
            .and_then(|target| target.provider.clone())
            .unwrap_or_else(|| "inherit".to_string());
        let model = target
            .and_then(|target| target.model.clone())
            .or_else(|| inherited_backend.model_label())
            .unwrap_or_else(|| "inherit".to_string());
        Ok(ResolvedSubagentBackend {
            backend: inherited_backend,
            provider,
            model,
        })
    }
}

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
    pub backend_resolver: Arc<dyn SubagentBackendResolver>,
    pub memory_handle: Arc<MemoryHandle>,
    pub session_manager: Arc<SessionManager>,
    pub workspace: Arc<WorkspaceMemory>,
    pub prompt_config: PromptRuntimeConfig,
    pub background_subagents: Arc<BackgroundSubAgentStore>,
    pub task_list_id: String,
    pub agent_definitions: AgentDefinitionCache,
    pub skill_manager: Arc<RwLock<SkillManager>>,
}

#[tool_spec(
    name = "spawn_agent",
    description = "Spawn a bounded worker sub-agent. Built-in names: general for shared-task reasoning, code-reviewer for independent code review, architect for architecture and trade-off analysis, and researcher for multi-source research with file or URL evidence. The specialist roles are read-only and cannot run shell commands, edit files, or spawn agents. Use explore_agent for generic repository inspection or plan_agent for implementation planning.",
    input_schema = {
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "instruction": { "type": "string" },
            "provider": {
                "type": "string",
                "description": "Optional provider override for this child. Omit to use the selected agent definition or inherit the parent backend."
            },
            "model": {
                "type": "string",
                "description": "Optional model override. Use model or provider:model; invocation override takes precedence over the agent definition."
            },
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
        let definition = resolve_spawn_agent_definition(&self.agent_definitions, &agent_label);
        let model_target = model_target_from_input(&i, Some(&definition))?;
        let start = BackgroundSubAgentStart {
            kind: SubAgentKind::General,
            agent_id,
            name: Some(name.to_string()),
            definition,
            model_target,
            parent_session_id: parent_session_id.map(str::to_string),
            instruction: instruction.to_string(),
            backend: self.backend.clone(),
            backend_resolver: self.backend_resolver.clone(),
            memory_handle: self.memory_handle.clone(),
            session_manager: self.session_manager.clone(),
            workspace: self.workspace.clone(),
            prompt_config: self.prompt_config.clone(),
            task_list_id: self.task_list_id.clone(),
            agent_definitions: self.agent_definitions.clone(),
            skill_manager: Some(self.skill_manager.clone()),
        };
        if i.get("run_in_background")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let task = self.background_subagents.start(start)?;
            return Ok(task.to_json());
        }
        let result = self.background_subagents.run(start).await?;
        Ok(json!({
            "agent_id": result.agent_id,
            "session_id": result.session_id,
            "name": name,
            "status": result.status,
            "summary": result.summary,
            "provider": result.provider,
            "model": result.model,
            "cache_hit_tokens": result.total_cache_hit_tokens,
            "cache_miss_tokens": result.total_cache_miss_tokens,
            "token_budget": result.token_budget,
            "token_budget_exhausted": result.token_budget_exhausted,
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
    pub backend_resolver: Arc<dyn SubagentBackendResolver>,
    pub memory_handle: Arc<MemoryHandle>,
    pub session_manager: Arc<SessionManager>,
    pub workspace: Arc<WorkspaceMemory>,
    pub prompt_config: PromptRuntimeConfig,
    pub background_subagents: Arc<BackgroundSubAgentStore>,
    pub task_list_id: String,
    pub agent_definitions: AgentDefinitionCache,
    pub skill_manager: Arc<RwLock<SkillManager>>,
}

#[tool_spec(
    name = "explore_agent",
    description = "Spawn a read-only exploration sub-agent for bounded independent sidecar repository inspection. The instruction must be self-contained and include all user constraints.",
    input_schema = {
        "type": "object",
        "properties": {
            "instruction": { "type": "string" },
            "provider": {
                "type": "string",
                "description": "Optional provider override for this child."
            },
            "model": {
                "type": "string",
                "description": "Optional model override. Use model or provider:model."
            },
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
        let definition = resolve_kind_definition(SubAgentKind::Explore);
        let model_target = model_target_from_input(&i, Some(&definition))?;
        let start = BackgroundSubAgentStart {
            kind: SubAgentKind::Explore,
            agent_id,
            name: None,
            definition,
            model_target,
            parent_session_id: parent_session_id.map(str::to_string),
            instruction: instruction.to_string(),
            backend: self.backend.clone(),
            backend_resolver: self.backend_resolver.clone(),
            memory_handle: self.memory_handle.clone(),
            session_manager: self.session_manager.clone(),
            workspace: self.workspace.clone(),
            prompt_config: self.prompt_config.clone(),
            task_list_id: self.task_list_id.clone(),
            agent_definitions: self.agent_definitions.clone(),
            skill_manager: Some(self.skill_manager.clone()),
        };
        if i.get("run_in_background")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let task = self.background_subagents.start(start)?;
            return Ok(task.to_json());
        }
        let result = self.background_subagents.run(start).await?;
        Ok(json!({
            "agent_id": result.agent_id,
            "session_id": result.session_id,
            "status": result.status,
            "summary": result.summary,
            "provider": result.provider,
            "model": result.model,
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
    pub backend_resolver: Arc<dyn SubagentBackendResolver>,
    pub memory_handle: Arc<MemoryHandle>,
    pub session_manager: Arc<SessionManager>,
    pub workspace: Arc<WorkspaceMemory>,
    pub prompt_config: PromptRuntimeConfig,
    pub background_subagents: Arc<BackgroundSubAgentStore>,
    pub task_list_id: String,
    pub agent_definitions: AgentDefinitionCache,
    pub skill_manager: Arc<RwLock<SkillManager>>,
}

#[tool_spec(
    name = "plan_agent",
    description = "Spawn a read-only planning sub-agent for bounded independent sidecar plan refinement. The instruction must be self-contained and include all user constraints.",
    input_schema = {
        "type": "object",
        "properties": {
            "instruction": { "type": "string" },
            "provider": {
                "type": "string",
                "description": "Optional provider override for this child."
            },
            "model": {
                "type": "string",
                "description": "Optional model override. Use model or provider:model."
            },
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
        let definition = resolve_kind_definition(SubAgentKind::Plan);
        let model_target = model_target_from_input(&i, Some(&definition))?;
        let start = BackgroundSubAgentStart {
            kind: SubAgentKind::Plan,
            agent_id,
            name: None,
            definition,
            model_target,
            parent_session_id: parent_session_id.map(str::to_string),
            instruction: instruction.to_string(),
            backend: self.backend.clone(),
            backend_resolver: self.backend_resolver.clone(),
            memory_handle: self.memory_handle.clone(),
            session_manager: self.session_manager.clone(),
            workspace: self.workspace.clone(),
            prompt_config: self.prompt_config.clone(),
            task_list_id: self.task_list_id.clone(),
            agent_definitions: self.agent_definitions.clone(),
            skill_manager: Some(self.skill_manager.clone()),
        };
        if i.get("run_in_background")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let task = self.background_subagents.start(start)?;
            return Ok(task.to_json());
        }
        let result = self.background_subagents.run(start).await?;
        Ok(json!({
            "agent_id": result.agent_id,
            "session_id": result.session_id,
            "status": result.status,
            "summary": result.summary,
            "provider": result.provider,
            "model": result.model,
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
    pub backend_resolver: Arc<dyn SubagentBackendResolver>,
    pub memory_handle: Arc<MemoryHandle>,
    pub session_manager: Arc<SessionManager>,
    pub workspace: Arc<WorkspaceMemory>,
    pub prompt_config: PromptRuntimeConfig,
    pub background_subagents: Arc<BackgroundSubAgentStore>,
    pub task_list_id: String,
    pub agent_definitions: AgentDefinitionCache,
    pub skill_manager: Arc<RwLock<SkillManager>>,
}

#[tool_spec(
    name = "team_create",
    description = "Launch up to 8 bounded sub-agents. All launch surfaces share the runtime's active-agent budget (three children by default). Each task may independently select kind, provider, and model.",
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
                        },
                        "provider": {
                            "type": "string",
                            "description": "Optional provider override for this sub-agent. Use with model to run one task on a different configured provider."
                        },
                        "model": {
                            "type": "string",
                            "description": "Optional model override for this sub-agent. Bare model uses the current provider; provider:model selects a configured provider."
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
            let control = self.background_subagents.clone();
            let start = BackgroundSubAgentStart {
                kind: task.kind,
                agent_id: next_subagent_id(task.kind, Some(&task.name)),
                name: Some(task.name.clone()),
                definition: task.definition,
                model_target: task.model_target,
                parent_session_id: parent_session_id.map(str::to_string),
                instruction: task.instruction,
                backend: self.backend.clone(),
                backend_resolver: self.backend_resolver.clone(),
                memory_handle: self.memory_handle.clone(),
                session_manager: self.session_manager.clone(),
                workspace: self.workspace.clone(),
                prompt_config: self.prompt_config.clone(),
                task_list_id: self.task_list_id.clone(),
                agent_definitions: self.agent_definitions.clone(),
                skill_manager: Some(self.skill_manager.clone()),
            };
            let task_name = task.name;

            async move {
                let result = control.run(start).await?;
                Ok::<_, ToolError>(serialize_team_result(&task_name, result))
            }
        });

        let results = stream::iter(runs)
            .buffered(self.background_subagents.max_active_subagents())
            .try_collect::<Vec<_>>()
            .await?;
        Ok(json!({ "team_results": results }))
    }
}

struct TeamTask {
    name: String,
    instruction: String,
    kind: SubAgentKind,
    definition: AgentDefinition,
    model_target: Option<SubagentProviderTarget>,
}

pub(crate) struct SubAgentResult {
    agent_id: String,
    session_id: String,
    pub(crate) status: &'static str,
    pub(crate) summary: String,
    pub(crate) provider: String,
    pub(crate) model: String,
    persistence_error: Option<String>,
    plan: Option<Vec<PlanStep>>,
    plan_explanation: Option<String>,
    request_user_input: Option<PendingUserInput>,
    pub(crate) total_input_tokens: u32,
    pub(crate) total_output_tokens: u32,
    pub(crate) total_cache_hit_tokens: u32,
    pub(crate) total_cache_miss_tokens: u32,
    pub(crate) token_budget: Option<u32>,
    pub(crate) token_budget_exhausted: bool,
}

#[cfg(test)]
#[path = "agent_test.rs"]
mod tests;
