use std::collections::BTreeSet;
use std::sync::atomic::Ordering;

use super::{
    CompletedInteractionSnapshot, InteractionKind, PendingApprovalSnapshot,
    PendingInteractionSnapshot, PlanningLifecycleSnapshot, RuntimeSnapshot, SkillPickerEntry,
    TuiApp,
};
use crate::agent::Agent;

impl TuiApp {
    pub fn sync_snapshot(&mut self, agent: &Agent) {
        let runtime_context = agent.shared_runtime_context();
        let existing_pending_approval_id = self
            .pending_command_approval()
            .and_then(|item| item.approval.as_ref())
            .map(|approval| approval.tool_use_id.clone());
        let existing_plan_completion = self
            .completed_interaction(InteractionKind::PlanApproval)
            .cloned();
        let existing_pending_plan_approval = self.pending_plan_approval_interaction().cloned();
        let existing_local_request_completion = self
            .snapshot
            .completed_interactions
            .iter()
            .find(|item| {
                item.kind == InteractionKind::RequestInput && item.source.as_deref().is_some()
            })
            .cloned();
        let existing_local_request_inputs = self
            .snapshot
            .pending_interactions
            .iter()
            .filter(|item| {
                item.kind == InteractionKind::RequestInput && item.source.as_deref().is_some()
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut pending_interactions = Vec::new();
        if let Some(question) = agent.pending_user_input.as_ref() {
            pending_interactions.push(PendingInteractionSnapshot {
                kind: InteractionKind::RequestInput,
                title: question.question.clone(),
                summary: question.note.clone().unwrap_or_default(),
                options: question.options.clone(),
                note: question.note.clone(),
                approval: None,
                source: None,
                created_at_epoch_seconds: None,
            });
        }
        if agent.pending_user_input.is_none() {
            pending_interactions.extend(existing_local_request_inputs);
        }
        if let Some(item) = existing_pending_plan_approval {
            pending_interactions.push(item);
        }
        if let Some(pending) = agent.pending_approval.as_ref() {
            pending_interactions.push(PendingInteractionSnapshot {
                kind: InteractionKind::Approval,
                title: "Pending Approval".to_string(),
                summary: pending.request.summary(),
                options: Vec::new(),
                note: None,
                approval: Some(PendingApprovalSnapshot {
                    tool_use_id: pending.tool_use_id.clone(),
                    command: pending.request.summary(),
                    allow_net: self.sandbox_network_access.load(Ordering::Relaxed)
                        || pending.request.allow_net,
                    payload: pending.request.clone(),
                }),
                source: None,
                created_at_epoch_seconds: None,
            });
        }
        let current_pending_approval_id = agent
            .pending_approval
            .as_ref()
            .map(|pending| pending.tool_use_id.clone());
        if current_pending_approval_id != existing_pending_approval_id {
            self.approval_picker_idx = 0;
        }
        let mut completed_interactions = Vec::new();
        if let Some(item) = agent.completed_user_input.as_ref() {
            completed_interactions.push(CompletedInteractionSnapshot {
                kind: InteractionKind::RequestInput,
                title: item.title.clone(),
                summary: item.summary.clone(),
                source: None,
                feedback: None,
                completed_at_epoch_seconds: None,
                plan_revision: None,
            });
        }
        if let Some(item) = agent.completed_approval.as_ref() {
            completed_interactions.push(CompletedInteractionSnapshot {
                kind: InteractionKind::Approval,
                title: item.title.clone(),
                summary: item.summary.clone(),
                source: None,
                feedback: None,
                completed_at_epoch_seconds: None,
                plan_revision: None,
            });
        }
        if let Some(item) = existing_local_request_completion {
            completed_interactions.push(item);
        }
        if let Some(item) = existing_plan_completion {
            completed_interactions.push(item);
        }
        for interaction in completed_interactions.iter() {
            self.ensure_completed_interaction_entry(
                interaction.kind,
                interaction.title.as_str(),
                interaction.summary.as_str(),
                interaction.source.as_deref(),
            );
        }
        let ext_counts = discover_extension_counts(&runtime_context.cwd, agent);
        let runtime_hook_count = self
            .hook_runtime
            .as_ref()
            .map(|runtime| runtime.hook_count())
            .unwrap_or(0);
        let planning_lifecycle = PlanningLifecycleSnapshot::from_interactions(
            &runtime_context.session_id,
            &pending_interactions,
            &completed_interactions,
        );
        self.snapshot = RuntimeSnapshot {
            cwd: runtime_context.cwd,
            branch: runtime_context.branch,
            session_id: runtime_context.session_id,
            history_len: runtime_context.history_len,
            total_input_tokens: runtime_context.total_input_tokens,
            total_output_tokens: runtime_context.total_output_tokens,
            total_cache_hit_tokens: runtime_context.total_cache_hit_tokens,
            total_cache_miss_tokens: runtime_context.total_cache_miss_tokens,
            context_window_tokens: runtime_context.budget.context_window_tokens,
            compact_threshold_tokens: runtime_context.budget.compact_threshold_tokens,
            reserved_output_tokens: runtime_context.budget.reserved_output_tokens,
            stable_instructions_budget: runtime_context.budget.stable_instructions_budget,
            workspace_prompt_budget: runtime_context.budget.workspace_prompt_budget,
            active_turn_budget: runtime_context.budget.active_turn_budget,
            compacted_history_budget: runtime_context.budget.compacted_history_budget,
            retrieved_memory_budget: runtime_context.budget.retrieved_memory_budget,
            remaining_input_budget: runtime_context.budget.remaining_input_budget,
            estimated_history_tokens: runtime_context.compaction.estimated_history_tokens,
            compaction_count: runtime_context.compaction.compaction_count,
            last_compaction_before_tokens: runtime_context.compaction.last_compaction_before_tokens,
            last_compaction_after_tokens: runtime_context.compaction.last_compaction_after_tokens,
            last_compaction_recent_files: runtime_context.compaction.last_compaction_recent_files,
            last_compaction_boundary_version: runtime_context
                .compaction
                .last_compaction_boundary_version,
            last_compaction_boundary_before_tokens: runtime_context
                .compaction
                .last_compaction_boundary_before_tokens,
            last_compaction_boundary_recent_file_count: runtime_context
                .compaction
                .last_compaction_boundary_recent_file_count,
            compaction_source_entries: runtime_context.compaction.source_entries,
            plan_steps: runtime_context.plan.steps,
            plan_explanation: runtime_context.plan.explanation,
            planning_lifecycle,
            pending_interactions,
            completed_interactions,
            todo_artifact_path: if agent.todo_state.is_some() {
                Some(
                    agent
                        .session_manager
                        .todo_file_path(&agent.session_id)
                        .display()
                        .to_string(),
                )
            } else {
                None
            },
            todo: runtime_context.todo,
            shared_tasks: runtime_context.shared_tasks,
            prompt_base_kind: runtime_context.prompt.base_prompt_kind,
            prompt_section_keys: runtime_context.prompt.section_keys,
            prompt_source_entries: runtime_context.prompt.source_entries,
            prompt_source_status_lines: runtime_context.prompt.source_status_lines,
            prompt_append_system_prompt: runtime_context.prompt.append_system_prompt,
            prompt_warnings: runtime_context.prompt.warnings,
            retrieval_source_entries: runtime_context.retrieval.entries,
            retrieval_orchestration: runtime_context.retrieval.orchestration,
            memory_selection: runtime_context.retrieval.memory_selection,
            context_observability: runtime_context.observability,
            assembly_entries: runtime_context.assembly.entries,
            extension_skill_count: agent.prompt_config().available_skills.len(),
            extension_skill_scopes: {
                agent
                    .prompt_config()
                    .available_skills
                    .iter()
                    .map(|s| s.scope.clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect()
            },
            extension_hook_count: ext_counts.0.max(runtime_hook_count),
            extension_agent_count: ext_counts.1,
            extension_agent_status_lines: ext_counts.2,
        };
        self.agent_execution_mode = agent.execution_mode;
        self.bash_approval_mode = agent.bash_approval_mode;
        self.populate_skill_picker_entries(agent);
        self.persist_runtime_state();
    }

    pub fn populate_skill_picker_entries(&mut self, agent: &Agent) {
        self.skill_picker_entries = agent
            .prompt_config()
            .available_skills
            .iter()
            .map(|s| SkillPickerEntry {
                name: s.name.clone(),
                title: s.title.clone().unwrap_or_else(|| s.name.clone()),
                scope: s.scope.clone(),
                enabled: !s.disable_model_invocation,
                disable_model_invocation: s.disable_model_invocation,
            })
            .collect();
        self.skill_picker_entries
            .sort_by(|a, b| a.name.cmp(&b.name));
    }
}

fn discover_extension_counts(cwd: &str, agent: &Agent) -> (usize, usize, Vec<String>) {
    let root = std::path::Path::new(cwd);
    let mut hook_registry = crate::hooks::HookRegistry::new();
    hook_registry.discover_repo_hooks(root);
    let records = agent
        .agent_definition_records()
        .into_iter()
        .filter(|record| record.source_path.starts_with(root))
        .collect();
    let agent_registry = crate::agents_ext::AgentRegistry::from_records(records, root);
    let agent_count = agent_registry.agents.len();
    let agent_status_lines = if agent_count == 0 {
        Vec::new()
    } else {
        agent_registry.status_lines()
    };
    (hook_registry.hooks.len(), agent_count, agent_status_lines)
}
