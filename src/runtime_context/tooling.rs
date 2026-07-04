use std::collections::HashMap;
use std::sync::{Arc, OnceLock, atomic::AtomicBool};

use rara_background_tasks::{
    BackgroundTaskListTool, BackgroundTaskStatusTool, BackgroundTaskStopTool, BackgroundTaskStore,
};
use rara_memory::vectordb::VectorDB;
use rara_tools::file::{
    FileReadState, ListFilesTool, MultiEditTool, ReadFileTool, ReplaceLinesTool, ReplaceTool,
    WriteFileTool,
};
use rara_tools::memory::SearchMemoryTool;
use rara_tools::patch::ApplyPatchTool;
use rara_tools::planning::{EnterPlanModeTool, ExitPlanModeTool};
use rara_tools::search::{GlobTool, GrepTool};
use rara_tools::tool::ToolManager;

use crate::llm::{EmbeddingBackend, LlmBackend};
use crate::lsp_manager::LspManager;
use crate::mcp_tool_cache::McpToolCache;
use crate::prompt::PromptRuntimeConfig;
use crate::sandbox::SandboxManager;
use crate::session::SessionManager;
use crate::skill::SkillManager;
use crate::tasklist::DEFAULT_TASK_LIST_ID;
use crate::tasklist::TaskListStore;
use crate::tools::agent::{
    AgentDefinitionCache, AgentTool, BackgroundSubAgentStore, ExploreAgentTool, PlanAgentTool,
    SubAgentListTool, SubAgentResumeTool, SubAgentStopTool, TeamCreateTool,
};
use crate::tools::bash::BashTool;
use crate::tools::context::RetrieveSessionContextTool;
use crate::tools::goal::{CreateGoalTool, GetGoalTool, UpdateGoalTool};
use crate::tools::lsp::LspDiagnosticsTool;
use crate::tools::mcp_tool_search::McpToolSearch;
use crate::tools::pty::{
    PtyKillTool, PtyListTool, PtyReadTool, PtySessionStore, PtyStartTool, PtyStatusTool,
    PtyStopTool, PtyWriteTool,
};
use crate::tools::skill::SkillTool;
use crate::tools::tasklist::{TaskCreateTool, TaskGetTool, TaskListTool, TaskUpdateTool};
use crate::tools::todo::TodoWriteTool;
use crate::tools::vector::{RememberExperienceTool, RetrieveExperienceTool};
use crate::tools::web::{WebFetchTool, WebSearchTool};
use crate::tools::workspace::UpdateProjectMemoryTool;
use crate::tui::state::GoalHandle;
use crate::workspace::WorkspaceMemory;

static BACKGROUND_SUBAGENTS: OnceLock<Arc<BackgroundSubAgentStore>> = OnceLock::new();

#[allow(clippy::too_many_arguments)]
// Tool manager construction is the runtime bootstrap composition point; the
// explicit arguments document which subsystems each tool may receive.
pub(super) fn create_full_tool_manager(
    backend: Arc<dyn LlmBackend>,
    embedding_backend: Arc<dyn EmbeddingBackend>,
    vdb: Arc<VectorDB>,
    session_manager: Arc<SessionManager>,
    workspace: Arc<WorkspaceMemory>,
    sandbox: Arc<SandboxManager>,
    skill_manager: Arc<SkillManager>,
    prompt_config: PromptRuntimeConfig,
    shell_env: Arc<HashMap<String, String>>,
    sandbox_network_access: Arc<AtomicBool>,
    goal_handle: GoalHandle,
    mcp_tool_cache: McpToolCache,
    lsp_manager: Arc<LspManager>,
    agent_definitions: AgentDefinitionCache,
) -> ToolManager {
    let mut tm = ToolManager::new();
    let vector_db_uri = vector_db_uri_for_workspace(&workspace);
    let background_tasks = Arc::new(
        BackgroundTaskStore::new(workspace.rara_dir.join("background-tasks"))
            .expect("background task store"),
    );
    let pty_sessions = Arc::new(
        PtySessionStore::new(workspace.rara_dir.join("pty-sessions")).expect("pty session store"),
    );
    let task_list_store = Arc::new(TaskListStore::new(workspace.rara_dir.join("tasks")));
    let task_list_id = DEFAULT_TASK_LIST_ID.to_string();
    let background_subagents = BACKGROUND_SUBAGENTS
        .get_or_init(|| Arc::new(BackgroundSubAgentStore::default()))
        .clone();
    let file_read_state = Arc::new(FileReadState::default());

    tm.register(Box::new(BashTool {
        sandbox: sandbox.clone(),
        background_tasks: background_tasks.clone(),
        base_env: shell_env.clone(),
        sandbox_network_access: sandbox_network_access.clone(),
    }));
    tm.register(Box::new(BackgroundTaskListTool {
        background_tasks: background_tasks.clone(),
    }));
    tm.register(Box::new(BackgroundTaskStatusTool {
        background_tasks: background_tasks.clone(),
    }));
    tm.register(Box::new(BackgroundTaskStopTool { background_tasks }));
    tm.register(Box::new(PtyStartTool {
        sessions: pty_sessions.clone(),
        sandbox: sandbox.clone(),
        base_env: shell_env.clone(),
        sandbox_network_access: sandbox_network_access.clone(),
    }));
    tm.register(Box::new(PtyReadTool {
        sessions: pty_sessions.clone(),
    }));
    tm.register(Box::new(PtyListTool {
        sessions: pty_sessions.clone(),
    }));
    tm.register(Box::new(PtyStatusTool {
        sessions: pty_sessions.clone(),
    }));
    tm.register(Box::new(PtyWriteTool {
        sessions: pty_sessions.clone(),
    }));
    tm.register(Box::new(PtyKillTool {
        sessions: pty_sessions.clone(),
    }));
    tm.register(Box::new(PtyStopTool {
        sessions: pty_sessions,
    }));
    tm.register(Box::new(ReadFileTool::new(file_read_state.clone())));
    tm.register(Box::new(ApplyPatchTool::new(file_read_state.clone())));
    tm.register(Box::new(WriteFileTool::new(file_read_state.clone())));
    tm.register(Box::new(ListFilesTool));
    tm.register(Box::new(ReplaceTool::new(file_read_state.clone())));
    tm.register(Box::new(ReplaceLinesTool::new(file_read_state.clone())));
    tm.register(Box::new(MultiEditTool::new(file_read_state)));
    tm.register(Box::new(WebFetchTool));
    tm.register(Box::new(WebSearchTool::from_env()));
    tm.register(Box::new(GlobTool));
    tm.register(Box::new(GrepTool));
    tm.register(Box::new(SearchMemoryTool {
        rara_home: workspace.rara_dir.clone(),
        vdb: Some(vdb.clone()),
        hook_callback: Some(Arc::new(crate::hook_runtime::global_dispatch_memory_query)),
    }));
    tm.register(Box::new(LspDiagnosticsTool::new(lsp_manager)));
    tm.register(Box::new(McpToolSearch::new(mcp_tool_cache)));
    tm.register(Box::new(EnterPlanModeTool));
    tm.register(Box::new(ExitPlanModeTool));
    tm.register(Box::new(TodoWriteTool));
    tm.register(Box::new(TaskCreateTool {
        store: task_list_store.clone(),
        default_task_list_id: task_list_id.clone(),
    }));
    tm.register(Box::new(TaskListTool {
        store: task_list_store.clone(),
        default_task_list_id: task_list_id.clone(),
    }));
    tm.register(Box::new(TaskUpdateTool {
        store: task_list_store.clone(),
        default_task_list_id: task_list_id.clone(),
    }));
    tm.register(Box::new(TaskGetTool {
        store: task_list_store,
        default_task_list_id: task_list_id.clone(),
    }));
    tm.register(Box::new(RememberExperienceTool {
        llm_backend: backend.clone(),
        embedding_backend: embedding_backend.clone(),
        vdb: vdb.clone(),
        db_uri: vector_db_uri.clone(),
    }));
    tm.register(Box::new(RetrieveExperienceTool {
        llm_backend: backend.clone(),
        embedding_backend: embedding_backend.clone(),
        vdb: vdb.clone(),
        db_uri: vector_db_uri,
    }));
    tm.register(Box::new(RetrieveSessionContextTool {
        embedding_backend: embedding_backend.clone(),
        session_manager: session_manager.clone(),
    }));
    tm.register(Box::new(UpdateProjectMemoryTool {
        workspace: workspace.clone(),
    }));
    tm.register(Box::new(SkillTool {
        skill_manager: skill_manager.clone(),
    }));
    tm.register(Box::new(AgentTool {
        backend: backend.clone(),
        embedding_backend: embedding_backend.clone(),
        vdb: vdb.clone(),
        session_manager: session_manager.clone(),
        workspace: workspace.clone(),
        prompt_config: prompt_config.clone(),
        background_subagents: background_subagents.clone(),
        task_list_id: task_list_id.clone(),
        agent_definitions: agent_definitions.clone(),
    }));
    tm.register(Box::new(ExploreAgentTool {
        backend: backend.clone(),
        embedding_backend: embedding_backend.clone(),
        vdb: vdb.clone(),
        session_manager: session_manager.clone(),
        workspace: workspace.clone(),
        prompt_config: prompt_config.clone(),
        background_subagents: background_subagents.clone(),
        task_list_id: task_list_id.clone(),
        agent_definitions: agent_definitions.clone(),
    }));
    tm.register(Box::new(PlanAgentTool {
        backend: backend.clone(),
        embedding_backend: embedding_backend.clone(),
        vdb: vdb.clone(),
        session_manager: session_manager.clone(),
        workspace: workspace.clone(),
        prompt_config: prompt_config.clone(),
        background_subagents: background_subagents.clone(),
        task_list_id: task_list_id.clone(),
        agent_definitions: agent_definitions.clone(),
    }));
    tm.register(Box::new(TeamCreateTool {
        backend,
        embedding_backend,
        vdb,
        session_manager: session_manager.clone(),
        workspace,
        prompt_config,
        task_list_id,
        agent_definitions,
    }));
    tm.register(Box::new(SubAgentResumeTool {
        background_subagents: background_subagents.clone(),
        session_manager: session_manager.clone(),
    }));
    tm.register(Box::new(SubAgentListTool {
        background_subagents: background_subagents.clone(),
        session_manager: session_manager.clone(),
    }));
    tm.register(Box::new(SubAgentStopTool {
        background_subagents,
    }));
    tm.register(Box::new(GetGoalTool {
        store: goal_handle.clone(),
    }));
    tm.register(Box::new(CreateGoalTool {
        store: goal_handle.clone(),
    }));
    tm.register(Box::new(UpdateGoalTool { store: goal_handle }));
    tm
}

pub(super) fn load_skill_manager(warnings: &mut Vec<String>) -> Arc<SkillManager> {
    let mut skill_manager = SkillManager::new();
    if let Err(err) = skill_manager.load_all() {
        warnings.push(format!("Skill loading failed: {err}"));
    }
    Arc::new(skill_manager)
}

pub(crate) fn vector_db_uri_for_workspace(workspace: &WorkspaceMemory) -> String {
    workspace.rara_dir.join("lancedb").display().to_string()
}
