use super::*;
use crate::context::{
    AssembledContext, AssembledTurnContext, ContextAssembler, RuntimeContextInputs,
    RuntimeInteractionInput,
};
use crate::protocol_sources::{PromptSourceRegistry, SkillSourceRegistry};
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
            retrieved_memory_candidates: &self.retrieved_memory_candidates,
            file_search_candidates: &self.file_search_candidates,
            mcp_resource_candidates: &self.mcp_resource_candidates,
            hook_output_candidates: &self.hook_output_candidates,
            graph_context_candidates: &self.graph_context_candidates,
            tool_result_projection_policy: self.tool_result_projection_policy(),
            tool_result_projection_report: self.last_tool_result_projection_report.clone(),
            agent_turn_trace: self.last_agent_turn_trace.clone(),
        }
    }

    pub(super) fn tool_result_projection_policy(&self) -> ToolResultProjectionPolicy {
        let mut policy = ToolResultProjectionPolicy::default()
            .for_provider_cache_edit(self.llm_backend.cache_profile().cache_edit);

        // Time-based trigger (Claude Code style):
        // If the session has been idle for a while, the provider cache is likely cold.
        // We can disable cache-edit eligibility to allow more aggressive microcompaction
        // since we'll pay the cache-miss penalty anyway.
        let idle_duration = self.last_interaction_time.elapsed();
        if idle_duration > std::time::Duration::from_secs(300) {
            policy.cache_edit_eligible = false;
        }

        policy
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

    pub fn set_prompt_source_registry(
        &mut self,
        prompt_source_registry: std::sync::Arc<PromptSourceRegistry>,
    ) {
        self.prompt_source_registry = Some(prompt_source_registry);
    }

    pub fn set_skill_source_registry(
        &mut self,
        skill_source_registry: std::sync::Arc<SkillSourceRegistry>,
    ) {
        self.skill_source_registry = Some(skill_source_registry);
    }

    pub fn prompt_config(&self) -> &PromptRuntimeConfig {
        &self.prompt_config
    }

    pub(crate) async fn refresh_protocol_prompt_sources_for_query(&mut self) {
        let Some(registry) = self.prompt_source_registry.as_ref() else {
            return;
        };
        self.prompt_config.protocol_prompt_sources = registry.list_prompt_sources_for_query().await;
    }

    pub(crate) async fn refresh_protocol_skill_sources_for_query(&mut self) {
        let Some(registry) = self.skill_source_registry.as_ref() else {
            return;
        };
        // For now we just emit the Injected events and snapshot them.
        // Integration with actual skill execution will follow.
        let _skills = registry.list_skills_for_query().await;
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
