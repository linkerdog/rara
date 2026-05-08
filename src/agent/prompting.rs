use super::*;
use crate::context::{
    AssembledContext, AssembledTurnContext, ContextAssembler, RuntimeContextInputs,
    RuntimeInteractionInput,
};
use crate::tool_result::ToolResultProjectionPolicy;

impl Agent {
    pub fn assemble_context(&self) -> AssembledContext {
        self.context_assembler().assemble({
            match self.execution_mode {
                AgentExecutionMode::Execute => PromptMode::Execute,
                AgentExecutionMode::Plan => PromptMode::Plan,
                AgentExecutionMode::Review => PromptMode::Review,
            }
        })
    }

    pub(super) fn context_assembler(&self) -> ContextAssembler<'_> {
        ContextAssembler::new(&self.workspace, &self.prompt_config)
    }

    pub fn assemble_turn_context(&self) -> AssembledTurnContext {
        let mode = match self.execution_mode {
            AgentExecutionMode::Execute => PromptMode::Execute,
            AgentExecutionMode::Plan => PromptMode::Plan,
            AgentExecutionMode::Review => PromptMode::Review,
        };
        self.context_assembler()
            .assemble_turn(mode, self.runtime_context_inputs())
    }

    pub fn assemble_runtime_context(&self) -> crate::context::SharedRuntimeContext {
        let mode = match self.execution_mode {
            AgentExecutionMode::Execute => PromptMode::Execute,
            AgentExecutionMode::Plan => PromptMode::Plan,
            AgentExecutionMode::Review => PromptMode::Review,
        };
        self.context_assembler()
            .assemble_runtime(mode, self.runtime_context_inputs())
    }

    fn runtime_context_inputs(&self) -> RuntimeContextInputs<'_> {
        let (cwd, branch) = self.workspace.get_env_info();
        RuntimeContextInputs {
            cwd,
            branch,
            session_id: self.session_id.clone(),
            history_len: self.history.len(),
            total_input_tokens: self.total_input_tokens,
            total_output_tokens: self.total_output_tokens,
            total_cache_hit_tokens: self.total_cache_hit_tokens,
            total_cache_miss_tokens: self.total_cache_miss_tokens,
            execution_mode: self.execution_mode_label().to_string(),
            plan_steps: self
                .current_plan
                .iter()
                .map(|step| (step.status.clone(), step.step.clone()))
                .collect(),
            plan_explanation: self.plan_explanation.clone(),
            todo_state: self.todo_state.clone(),
            compact_state: self.compact_state.clone(),
            history: &self.history,
            vdb_uri: self.vdb.uri(),
            pending_interactions: self.pending_runtime_interactions(),
            skill_listing: prompt::render_skill_listing(&self.prompt_config.available_skills),
            retrieved_memory_candidates: self.retrieved_memory_candidates.clone(),
            file_search_candidates: self.file_search_candidates.clone(),
            mcp_resource_candidates: self.mcp_resource_candidates.clone(),
            tool_result_projection_policy: ToolResultProjectionPolicy::default(),
            tool_result_projection_report: self.last_tool_result_projection_report.clone(),
            agent_turn_trace: self.last_agent_turn_trace.clone(),
        }
    }

    pub fn build_system_prompt(&self) -> String {
        self.assemble_context().effective_prompt.text
    }

    pub fn effective_prompt(&self) -> prompt::EffectivePrompt {
        self.assemble_context().effective_prompt
    }

    pub fn set_prompt_config(&mut self, prompt_config: PromptRuntimeConfig) {
        self.prompt_config = prompt_config;
    }

    pub fn prompt_config(&self) -> &PromptRuntimeConfig {
        &self.prompt_config
    }

    pub fn set_cancellation_token(
        &mut self,
        cancellation_token: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ) {
        self.cancellation_token = cancellation_token;
    }

    fn pending_runtime_interactions(&self) -> Vec<RuntimeInteractionInput> {
        let mut interactions = Vec::new();

        if let Some(question) = self.pending_user_input.as_ref() {
            interactions.push(RuntimeInteractionInput {
                kind: "request_input".to_string(),
                title: question.question.clone(),
                summary: question.note.clone().unwrap_or_default(),
                source: None,
            });
        }

        if let Some(approval) = self.pending_approval.as_ref() {
            interactions.push(RuntimeInteractionInput {
                kind: "approval".to_string(),
                title: "Pending Approval".to_string(),
                summary: approval.request.summary(),
                source: None,
            });
        }

        interactions
    }
}
