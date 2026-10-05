use super::Agent;

impl Agent {
    /// Start a prepared thread without carrying conversation state across identities.
    /// Workspace services, permissions, and stable prompt configuration stay attached.
    pub(crate) fn reset_for_new_thread(&mut self, session_id: String) {
        self.session_id = session_id;
        self.history.clear();
        self.total_input_tokens = 0;
        self.total_output_tokens = 0;
        self.total_cache_hit_tokens = 0;
        self.total_cache_miss_tokens = 0;
        self.aux_total_cache_hit_tokens = 0;
        self.aux_total_cache_miss_tokens = 0;
        self.token_budget_exhausted = false;
        self.current_plan.clear();
        self.plan_explanation = None;
        self.pending_user_input = None;
        self.pending_approval = None;
        self.completed_user_input = None;
        self.completed_approval = None;
        self.todo_state = None;
        self.compact_state = super::CompactState {
            context_window_tokens: self.compact_state.context_window_tokens,
            compact_threshold_tokens: self.compact_state.compact_threshold_tokens,
            reserved_output_tokens: self.compact_state.reserved_output_tokens,
            ..Default::default()
        };
        self.retrieved_memory_candidates.clear();
        self.file_search_candidates.clear();
        self.mcp_resource_candidates.clear();
        self.hook_output_candidates.clear();
        self.graph_context_candidates.clear();
        self.last_tool_result_projection_report = Default::default();
        self.last_agent_turn_trace = Default::default();
        self.last_query_report = Default::default();
        self.pending_inference_agent = None;
        self.inference_context = None;
        self.summary_prefix = None;
        self.stable_tool_schemas = None;
        self.inspection_progress = Default::default();
        self.last_query_plan_updated = false;
        self.recent_tool_calls.clear();
        self.pending_plan_exit_tool_id = None;
        self.cancellation_token = None;
        self.runtime_turn_id = None;
        self.plugin_session_start_hooks_ran = false;
        if let Some(runtime) = &self.hook_runtime {
            runtime.blocking_drain_outputs();
        }
    }
}
