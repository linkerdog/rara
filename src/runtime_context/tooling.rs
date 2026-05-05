use std::collections::HashMap;
use std::sync::{Arc, atomic::AtomicBool};

use crate::llm::LlmBackend;
use crate::prompt::PromptRuntimeConfig;
use crate::sandbox::SandboxManager;
use crate::session::SessionManager;
use crate::skill::SkillManager;
use crate::tool::ToolManager;
use crate::tools::agent::{AgentTool, ExploreAgentTool, PlanAgentTool, TeamCreateTool};
use crate::tools::bash::{
    BackgroundTaskListTool, BackgroundTaskStatusTool, BackgroundTaskStopTool, BackgroundTaskStore,
    BashTool,
};
use crate::tools::context::RetrieveSessionContextTool;
use crate::tools::file::{
    FileReadState, ListFilesTool, ReadFileTool, ReplaceLinesTool, ReplaceTool, WriteFileTool,
};
use crate::tools::patch::ApplyPatchTool;
use crate::tools::planning::{EnterPlanModeTool, ExitPlanModeTool};
use crate::tools::pty::{
    PtyKillTool, PtyListTool, PtyReadTool, PtySessionStore, PtyStartTool, PtyStatusTool,
    PtyStopTool, PtyWriteTool,
};
use crate::tools::search::{GlobTool, GrepTool};
use crate::tools::skill::SkillTool;
use crate::tools::todo::TodoWriteTool;
use crate::tools::vector::{RememberExperienceTool, RetrieveExperienceTool};
use crate::tools::web::{WebFetchTool, WebSearchTool};
use crate::tools::workspace::UpdateProjectMemoryTool;
use crate::vectordb::VectorDB;
use crate::workspace::WorkspaceMemory;

pub(super) fn create_full_tool_manager(
    backend: Arc<dyn LlmBackend>,
    vdb: Arc<VectorDB>,
    session_manager: Arc<SessionManager>,
    workspace: Arc<WorkspaceMemory>,
    sandbox: Arc<SandboxManager>,
    skill_manager: Arc<SkillManager>,
    prompt_config: PromptRuntimeConfig,
    shell_env: Arc<HashMap<String, String>>,
    sandbox_network_access: Arc<AtomicBool>,
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
    tm.register(Box::new(ReplaceLinesTool::new(file_read_state)));
    tm.register(Box::new(WebFetchTool));
    tm.register(Box::new(WebSearchTool::from_env()));
    tm.register(Box::new(GlobTool));
    tm.register(Box::new(GrepTool));
    tm.register(Box::new(EnterPlanModeTool));
    tm.register(Box::new(ExitPlanModeTool));
    tm.register(Box::new(TodoWriteTool));
    tm.register(Box::new(RememberExperienceTool {
        backend: backend.clone(),
        vdb: vdb.clone(),
        db_uri: vector_db_uri.clone(),
    }));
    tm.register(Box::new(RetrieveExperienceTool {
        backend: backend.clone(),
        vdb: vdb.clone(),
        db_uri: vector_db_uri,
    }));
    tm.register(Box::new(RetrieveSessionContextTool {
        backend: backend.clone(),
        vdb: vdb.clone(),
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
        vdb: vdb.clone(),
        session_manager: session_manager.clone(),
        workspace: workspace.clone(),
        prompt_config: prompt_config.clone(),
    }));
    tm.register(Box::new(ExploreAgentTool {
        backend: backend.clone(),
        vdb: vdb.clone(),
        session_manager: session_manager.clone(),
        workspace: workspace.clone(),
        prompt_config: prompt_config.clone(),
    }));
    tm.register(Box::new(PlanAgentTool {
        backend: backend.clone(),
        vdb: vdb.clone(),
        session_manager: session_manager.clone(),
        workspace: workspace.clone(),
        prompt_config: prompt_config.clone(),
    }));
    tm.register(Box::new(TeamCreateTool {
        backend,
        vdb,
        session_manager,
        workspace,
        prompt_config,
    }));
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
