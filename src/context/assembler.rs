use serde_json::Value;

use crate::agent::{CompactState, Message, PlanStepStatus};
use crate::context::assembly_view::assemble_context_view;
use crate::context::compaction_view::compaction_source_entries;
use crate::context::memory_selection::memory_selection;
use crate::context::retrieval_view::{retrieval_orchestration_view, retrieval_source_entries};
use crate::context::{
    CompactionContextView, ContextBudgetView, ContextCacheObservationView,
    ContextCompactionObservationView, ContextObservabilityView, MicrocompactProjectionContextView,
    PlanContextView, PromptContextView, RetrievalCandidate, RetrievalContextView,
    RetrievalObservationView, RetrievalRequest, RetrievedMemoryCandidate, SharedRuntimeContext,
    TodoContextView, retrieval_candidates,
};
use crate::llm::{ContextBudget, LlmBackend};
use crate::prompt::{self, EffectivePrompt, PromptMode, PromptRuntimeConfig};
use crate::todo::TodoState;
use crate::tool_result::{ToolResultProjectionPolicy, ToolResultProjectionReport};
use crate::workspace::WorkspaceMemory;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembledContext {
    pub effective_prompt: EffectivePrompt,
    pub compact_instruction: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembledTurnContext {
    pub prompt: AssembledContext,
    pub runtime: SharedRuntimeContext,
}

#[derive(Debug, Clone)]
pub struct RuntimeContextInputs<'a> {
    pub cwd: String,
    pub branch: String,
    pub session_id: String,
    pub history_len: usize,
    pub total_input_tokens: u32,
    pub total_output_tokens: u32,
    pub total_cache_hit_tokens: u32,
    pub total_cache_miss_tokens: u32,
    pub execution_mode: String,
    pub plan_steps: Vec<(PlanStepStatus, String)>,
    pub plan_explanation: Option<String>,
    pub todo_state: Option<TodoState>,
    pub compact_state: CompactState,
    pub history: &'a [Message],
    pub vdb_uri: &'a str,
    pub pending_interactions: Vec<RuntimeInteractionInput>,
    pub skill_listing: Option<String>,
    pub retrieved_memory_candidates: Vec<RetrievedMemoryCandidate>,
    pub file_search_candidates: Vec<RetrievalCandidate>,
    pub tool_result_projection_policy: ToolResultProjectionPolicy,
    pub tool_result_projection_report: ToolResultProjectionReport,
}

#[derive(Debug, Clone)]
pub struct RuntimeInteractionInput {
    pub kind: String,
    pub title: String,
    pub summary: String,
    pub source: Option<String>,
}

impl AssembledContext {
    pub fn system_prompt(&self) -> &str {
        &self.effective_prompt.text
    }
}

#[derive(Clone, Copy)]
pub struct ContextAssembler<'a> {
    workspace: &'a WorkspaceMemory,
    runtime: &'a PromptRuntimeConfig,
}

impl<'a> ContextAssembler<'a> {
    pub fn new(workspace: &'a WorkspaceMemory, runtime: &'a PromptRuntimeConfig) -> Self {
        Self { workspace, runtime }
    }

    pub fn assemble(&self, mode: PromptMode) -> AssembledContext {
        AssembledContext {
            effective_prompt: prompt::build_effective_prompt(self.workspace, self.runtime, mode),
            compact_instruction: prompt::build_compact_instruction(self.runtime),
        }
    }

    pub fn effective_prompt(&self, mode: PromptMode) -> EffectivePrompt {
        self.assemble(mode).effective_prompt
    }

    pub fn system_prompt(&self, mode: PromptMode) -> String {
        self.assemble(mode).effective_prompt.text
    }

    pub fn compact_instruction(&self) -> String {
        prompt::build_compact_instruction(self.runtime)
    }

    pub fn assemble_turn(
        &self,
        mode: PromptMode,
        inputs: RuntimeContextInputs<'_>,
    ) -> AssembledTurnContext {
        let prompt = self.assemble(mode);
        let runtime =
            self.assemble_runtime_from_effective_prompt(prompt.effective_prompt.clone(), inputs);
        AssembledTurnContext { prompt, runtime }
    }

    pub fn assemble_runtime(
        &self,
        mode: PromptMode,
        inputs: RuntimeContextInputs<'_>,
    ) -> SharedRuntimeContext {
        let effective_prompt = self.effective_prompt(mode);
        self.assemble_runtime_from_effective_prompt(effective_prompt, inputs)
    }

    fn assemble_runtime_from_effective_prompt(
        &self,
        effective_prompt: EffectivePrompt,
        inputs: RuntimeContextInputs<'_>,
    ) -> SharedRuntimeContext {
        let system_prompt_budget = estimate_text_tokens(effective_prompt.text.as_str());
        let stable_instructions_budget = system_prompt_budget;
        let workspace_prompt_budget = effective_prompt
            .sources
            .iter()
            .filter(|source| matches!(source.kind_label(), "project_instruction" | "local_memory"))
            .map(|source| estimate_text_tokens(source.content.as_str()))
            .sum();
        let retrieval_entries = retrieval_source_entries(
            self.workspace,
            effective_prompt.sources.as_slice(),
            inputs.history,
            inputs.session_id.as_str(),
            inputs.vdb_uri,
        );
        let mut compaction = CompactionContextView::from_compact_state(&inputs.compact_state);
        compaction.source_entries = compaction_source_entries(inputs.history);
        let compacted_history_budget = compaction
            .source_entries
            .iter()
            .map(|entry| estimate_text_tokens(entry.detail.as_str()))
            .sum();
        let active_turn_budget = active_turn_budget(
            inputs.plan_explanation.as_deref(),
            inputs.plan_steps.as_slice(),
            inputs.pending_interactions.as_slice(),
            inputs.history,
        );
        let selection_budget = inputs.compact_state.context_window_tokens.map(|window| {
            window
                .saturating_sub(inputs.compact_state.reserved_output_tokens)
                .saturating_sub(system_prompt_budget)
                .saturating_sub(active_turn_budget)
                .saturating_sub(compacted_history_budget)
        });
        let retrieval_query = latest_user_request(inputs.history).unwrap_or_default();
        let retrieval_request = RetrievalRequest {
            query: retrieval_query.as_str(),
            session_id: inputs.session_id.as_str(),
            history: inputs.history,
            vdb_uri: inputs.vdb_uri,
        };
        let retrieval_candidates = retrieval_candidates(
            &retrieval_request,
            inputs.retrieved_memory_candidates.as_slice(),
            inputs.file_search_candidates.as_slice(),
        );
        let memory_selection = memory_selection(
            effective_prompt.sources.as_slice(),
            inputs.plan_explanation.as_deref(),
            inputs.plan_steps.as_slice(),
            inputs.pending_interactions.as_slice(),
            compaction.source_entries.as_slice(),
            inputs.history,
            retrieval_candidates.as_slice(),
            selection_budget,
        );
        let retrieval = RetrievalContextView {
            orchestration: retrieval_orchestration_view(
                inputs.session_id.as_str(),
                retrieval_query.as_str(),
                retrieval_entries.as_slice(),
                &memory_selection,
            ),
            entries: retrieval_entries,
            memory_selection,
        };
        let observability = ContextObservabilityView {
            cache: ContextCacheObservationView::from_usage(
                inputs.total_cache_hit_tokens,
                inputs.total_cache_miss_tokens,
            ),
            compaction: ContextCompactionObservationView::from_compaction(&compaction),
            microcompact: MicrocompactProjectionContextView::from_report(
                &inputs.tool_result_projection_policy,
                &inputs.tool_result_projection_report,
            ),
            retrieval: RetrievalObservationView::from_orchestration(&retrieval.orchestration),
        };
        let retrieved_memory_budget = retrieval
            .memory_selection
            .selected_items
            .iter()
            .filter(|item| crate::context::is_retrieved_memory_kind(item.kind.as_str()))
            .map(|item| item.budget_impact_tokens.unwrap_or_default())
            .sum();
        let assembly = assemble_context_view(
            &effective_prompt,
            inputs.plan_explanation.as_deref(),
            inputs.plan_steps.as_slice(),
            inputs.pending_interactions.as_slice(),
            compaction.source_entries.as_slice(),
            retrieval.memory_selection.selected_items.as_slice(),
            retrieval.memory_selection.available_items.as_slice(),
            retrieval.memory_selection.dropped_items.as_slice(),
            inputs.history,
            inputs.skill_listing.as_deref(),
        );

        SharedRuntimeContext {
            cwd: inputs.cwd,
            branch: inputs.branch,
            session_id: inputs.session_id,
            history_len: inputs.history_len,
            total_input_tokens: inputs.total_input_tokens,
            total_output_tokens: inputs.total_output_tokens,
            total_cache_hit_tokens: inputs.total_cache_hit_tokens,
            total_cache_miss_tokens: inputs.total_cache_miss_tokens,
            budget: ContextBudgetView::from_compact_state(
                &inputs.compact_state,
                system_prompt_budget,
                stable_instructions_budget,
                workspace_prompt_budget,
                active_turn_budget,
                compacted_history_budget,
                retrieved_memory_budget,
            ),
            assembly,
            prompt: PromptContextView::from_effective_prompt(
                effective_prompt,
                self.runtime.append_system_prompt.clone(),
                self.runtime.warnings.clone(),
            ),
            plan: PlanContextView::from_agent_state(
                inputs.execution_mode.as_str(),
                inputs.plan_steps.into_iter(),
                inputs.plan_explanation,
            ),
            todo: TodoContextView::from_state(inputs.todo_state),
            compaction,
            retrieval,
            observability,
        }
    }

    pub fn budget_for(
        &self,
        backend: &dyn LlmBackend,
        history: &[Message],
        tools: &[Value],
    ) -> Option<ContextBudget> {
        backend.context_budget(history, tools)
    }
}

fn active_turn_budget(
    plan_explanation: Option<&str>,
    plan_steps: &[(PlanStepStatus, String)],
    pending_interactions: &[RuntimeInteractionInput],
    history: &[Message],
) -> usize {
    let plan_budget = plan_explanation
        .map(estimate_text_tokens)
        .unwrap_or_default()
        + plan_steps
            .iter()
            .map(|(_, step)| estimate_text_tokens(step.as_str()))
            .sum::<usize>();
    let interaction_budget = pending_interactions
        .iter()
        .map(|interaction| {
            estimate_text_tokens(interaction.title.as_str())
                + estimate_text_tokens(interaction.summary.as_str())
        })
        .sum::<usize>();
    let latest_request_budget = latest_user_request(history)
        .map(|value| estimate_text_tokens(value.as_str()))
        .unwrap_or_default();
    let tool_budget = latest_tool_results(history)
        .into_iter()
        .map(|(_, detail)| estimate_text_tokens(detail.as_str()))
        .sum::<usize>();

    plan_budget + interaction_budget + latest_request_budget + tool_budget
}

pub(crate) fn latest_user_request(history: &[Message]) -> Option<String> {
    history
        .iter()
        .rev()
        .filter(|message| message.role == "user")
        .find_map(extract_latest_text)
}

pub(crate) fn latest_tool_results(history: &[Message]) -> Vec<(String, String)> {
    history
        .iter()
        .rev()
        .find(|message| message.role == "user" && message.content.as_array().is_some())
        .and_then(|message| message.content.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    (item.get("type").and_then(Value::as_str) == Some("tool_result")).then(|| {
                        let tool_id = item
                            .get("tool_use_id")
                            .and_then(Value::as_str)
                            .unwrap_or("tool_result")
                            .to_string();
                        let content = item
                            .get("content")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        (format!("Tool Result {tool_id}"), content)
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn extract_latest_text(message: &Message) -> Option<String> {
    if let Some(text) = message.content.as_str() {
        let trimmed = text.trim();
        return (!trimmed.is_empty()).then(|| trimmed.to_string());
    }
    message.content.as_array().and_then(|items| {
        items
            .iter()
            .rev()
            .find_map(|item| {
                (item.get("type").and_then(Value::as_str) == Some("text"))
                    .then(|| item.get("text").and_then(Value::as_str))
                    .flatten()
            })
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    })
}

pub(crate) fn estimate_text_tokens(text: &str) -> usize {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        0
    } else {
        // Use a conservative text-to-token estimate so local/smaller-context
        // models do not silently overrun the remaining input budget.
        trimmed.len().div_ceil(3)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use anyhow::Result;
    use async_trait::async_trait;
    use rara_config::RaraConfig;
    use serde_json::json;

    use super::*;
    use crate::context::RETRIEVED_WORKSPACE_MEMORY_KIND;
    use crate::llm::{ContentBlock, LlmResponse};

    struct BudgetBackend {
        budget: Option<ContextBudget>,
    }

    #[async_trait]
    impl LlmBackend for BudgetBackend {
        async fn ask(&self, _messages: &[Message], _tools: &[Value]) -> Result<LlmResponse> {
            Ok(LlmResponse {
                content: vec![ContentBlock::Text {
                    text: "ok".to_string(),
                }],
                stop_reason: Some("end_turn".to_string()),
                usage: None,
            })
        }

        async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
            Ok(vec![0.0; 8])
        }

        async fn summarize(&self, _messages: &[Message], _instruction: &str) -> Result<String> {
            Ok("summary".to_string())
        }

        fn context_budget(&self, _messages: &[Message], _tools: &[Value]) -> Option<ContextBudget> {
            self.budget
        }
    }

    fn test_workspace() -> WorkspaceMemory {
        WorkspaceMemory::from_paths(PathBuf::from("/repo"), PathBuf::from("/repo/.rara"))
    }

    #[test]
    fn assemble_keeps_prompt_and_compact_instruction_together() {
        let workspace = test_workspace();
        let runtime = PromptRuntimeConfig {
            append_system_prompt: Some("appendix".to_string()),
            compact_prompt: Some("compact me".to_string()),
            ..PromptRuntimeConfig::default()
        };

        let assembled = ContextAssembler::new(&workspace, &runtime).assemble(PromptMode::Plan);

        assert!(assembled.system_prompt().contains("appendix"));
        assert_eq!(assembled.compact_instruction, "compact me");
        assert!(
            assembled
                .effective_prompt
                .section_keys
                .contains(&"append_system_prompt")
        );
    }

    #[test]
    fn assemble_runtime_collects_budget_and_runtime_views() {
        let workspace = test_workspace();
        let runtime = PromptRuntimeConfig {
            append_system_prompt: Some("appendix".to_string()),
            warnings: vec!["missing prompt file".to_string()],
            ..PromptRuntimeConfig::default()
        };

        let history = vec![Message {
            role: "assistant".to_string(),
            content: json!([{"type":"compacted_summary","text":"summary"}]),
        }];

        let runtime_context = ContextAssembler::new(&workspace, &runtime).assemble_runtime(
            PromptMode::Plan,
            RuntimeContextInputs {
                cwd: "repo".to_string(),
                branch: "main".to_string(),
                session_id: "session-1".to_string(),
                history_len: 3,
                total_input_tokens: 11,
                total_output_tokens: 7,
                total_cache_hit_tokens: 8,
                total_cache_miss_tokens: 2,
                execution_mode: "plan".to_string(),
                plan_steps: vec![(PlanStepStatus::Pending, "inspect bootstrap".to_string())],
                plan_explanation: Some("Keep one assembly path.".to_string()),
                todo_state: None,
                compact_state: crate::agent::CompactState {
                    estimated_history_tokens: 1234,
                    context_window_tokens: Some(8192),
                    compact_threshold_tokens: 7000,
                    reserved_output_tokens: 1024,
                    ..Default::default()
                },
                history: &history,
                vdb_uri: "memory://vdb",
                pending_interactions: Vec::new(),
                skill_listing: None,
                retrieved_memory_candidates: Vec::new(),
                file_search_candidates: vec![],
                tool_result_projection_policy: ToolResultProjectionPolicy::default(),
                tool_result_projection_report: ToolResultProjectionReport {
                    original_chars: 60_000,
                    projected_chars: 40_000,
                    cleared_results: 2,
                    kept_results: 6,
                },
            },
        );

        assert_eq!(runtime_context.session_id, "session-1");
        assert_eq!(runtime_context.budget.context_window_tokens, Some(8192));
        assert_eq!(runtime_context.budget.compact_threshold_tokens, 7000);
        assert_eq!(runtime_context.plan.execution_mode, "plan");
        assert_eq!(runtime_context.plan.steps.len(), 1);
        assert_eq!(
            runtime_context.prompt.warnings,
            vec!["missing prompt file".to_string()]
        );
        assert_eq!(
            runtime_context.prompt.append_system_prompt.as_deref(),
            Some("appendix")
        );
        assert_eq!(runtime_context.retrieval.entries.len(), 3);
        assert_eq!(runtime_context.compaction.source_entries.len(), 1);
        assert_eq!(
            runtime_context.observability.cache.hit_rate_basis_points,
            Some(8_000)
        );
        assert_eq!(
            runtime_context.observability.microcompact.saved_chars,
            20_000
        );
        assert_eq!(
            runtime_context.observability.microcompact.cleared_results,
            2
        );
        assert_eq!(
            runtime_context.observability.retrieval.provider_count,
            runtime_context.retrieval.entries.len()
        );
    }

    #[test]
    fn assemble_turn_keeps_prompt_and_runtime_views_aligned() {
        let workspace = test_workspace();
        let runtime = PromptRuntimeConfig {
            append_system_prompt: Some("appendix".to_string()),
            warnings: vec!["missing prompt file".to_string()],
            ..PromptRuntimeConfig::default()
        };
        let history = vec![Message {
            role: "user".to_string(),
            content: json!([{"type":"text","text":"hello"}]),
        }];

        let assembled = ContextAssembler::new(&workspace, &runtime).assemble_turn(
            PromptMode::Plan,
            RuntimeContextInputs {
                cwd: "repo".to_string(),
                branch: "main".to_string(),
                session_id: "session-1".to_string(),
                history_len: history.len(),
                total_input_tokens: 11,
                total_output_tokens: 7,
                total_cache_hit_tokens: 0,
                total_cache_miss_tokens: 0,
                execution_mode: "plan".to_string(),
                plan_steps: vec![(PlanStepStatus::Pending, "inspect bootstrap".to_string())],
                plan_explanation: Some("Keep one assembly path.".to_string()),
                todo_state: None,
                compact_state: crate::agent::CompactState {
                    estimated_history_tokens: 1234,
                    context_window_tokens: Some(8192),
                    compact_threshold_tokens: 7000,
                    reserved_output_tokens: 1024,
                    ..Default::default()
                },
                history: &history,
                vdb_uri: "memory://vdb",
                pending_interactions: Vec::new(),
                skill_listing: None,
                retrieved_memory_candidates: Vec::new(),
                file_search_candidates: vec![],
                tool_result_projection_policy: ToolResultProjectionPolicy::default(),
                tool_result_projection_report: ToolResultProjectionReport::default(),
            },
        );

        assert!(assembled.prompt.system_prompt().contains("appendix"));
        assert_eq!(
            assembled.runtime.prompt.append_system_prompt.as_deref(),
            Some("appendix")
        );
        assert_eq!(
            assembled.runtime.prompt.warnings,
            vec!["missing prompt file".to_string()]
        );
        assert_eq!(assembled.runtime.plan.execution_mode, "plan");
        assert_eq!(assembled.runtime.session_id, "session-1");
    }

    #[test]
    fn assemble_runtime_ranks_retrieval_candidates_against_selection_budget() {
        let workspace = test_workspace();
        let runtime = PromptRuntimeConfig::default();
        let history = vec![
            Message {
                role: "assistant".to_string(),
                content: json!([
                    {
                        "type": "tool_use",
                        "id": "tool-retrieve-1",
                        "name": "retrieve_experience",
                        "input": { "query": "bootstrap contract" }
                    },
                    {
                        "type": "tool_use",
                        "id": "tool-retrieve-2",
                        "name": "retrieve_session_context",
                        "input": { "query": "previous auth flow" }
                    }
                ]),
            },
            Message {
                role: "user".to_string(),
                content: json!([
                    {
                        "type": "tool_result",
                        "tool_use_id": "tool-retrieve-1",
                        "content": "Tool retrieve_experience completed with relevant_experiences.\nPayload:\n{\n  \"relevant_experiences\": [\n    \"Prefer one shared bootstrap path.\",\n    \"Keep session restore aligned with direct execution.\"\n  ]\n}"
                    },
                    {
                        "type": "tool_result",
                        "tool_use_id": "tool-retrieve-2",
                        "content": "Tool retrieve_session_context completed with status, summary.\nPayload:\n{\n  \"status\": \"ok\",\n  \"summary\": \"Auth picker already moved behind the shared runtime bootstrap.\"\n}"
                    }
                ]),
            },
        ];

        let runtime_context = ContextAssembler::new(&workspace, &runtime).assemble_runtime(
            PromptMode::Plan,
            RuntimeContextInputs {
                cwd: "repo".to_string(),
                branch: "main".to_string(),
                session_id: "session-1".to_string(),
                history_len: history.len(),
                total_input_tokens: 11,
                total_output_tokens: 7,
                total_cache_hit_tokens: 0,
                total_cache_miss_tokens: 0,
                execution_mode: "plan".to_string(),
                plan_steps: Vec::new(),
                plan_explanation: None,
                todo_state: None,
                compact_state: crate::agent::CompactState {
                    estimated_history_tokens: 1234,
                    context_window_tokens: Some(1_500),
                    compact_threshold_tokens: 1_420,
                    reserved_output_tokens: 1_024,
                    ..Default::default()
                },
                history: &history,
                vdb_uri: "memory://vdb",
                pending_interactions: Vec::new(),
                skill_listing: None,
                retrieved_memory_candidates: Vec::new(),
                file_search_candidates: vec![],
                tool_result_projection_policy: ToolResultProjectionPolicy::default(),
                tool_result_projection_report: ToolResultProjectionReport::default(),
            },
        );

        let selected_kinds = runtime_context
            .retrieval
            .memory_selection
            .selected_items
            .iter()
            .map(|item| item.kind.as_str())
            .collect::<Vec<_>>();
        let dropped_kinds = runtime_context
            .retrieval
            .memory_selection
            .dropped_items
            .iter()
            .map(|item| item.kind.as_str())
            .collect::<Vec<_>>();
        assert!(!selected_kinds.contains(&"retrieved_workspace_memory"));
        assert!(!selected_kinds.contains(&"retrieved_thread_context"));
        assert!(dropped_kinds.contains(&"retrieved_workspace_memory"));
        assert!(dropped_kinds.contains(&"retrieved_thread_context"));
        assert!(
            runtime_context
                .retrieval
                .memory_selection
                .dropped_items
                .iter()
                .any(|item| {
                    matches!(
                        item.kind.as_str(),
                        "retrieved_workspace_memory" | "retrieved_thread_context"
                    ) && item
                        .dropped_reason
                        .as_ref()
                        .is_some_and(|r| r.reason().contains("memory-selection budget"))
                })
        );
    }

    #[test]
    fn assemble_runtime_places_selected_retrieved_memory_in_active_memory_inputs() {
        let workspace = test_workspace();
        let runtime = PromptRuntimeConfig::default();
        let history = vec![Message {
            role: "user".to_string(),
            content: json!([{"type":"text","text":"Where is the reference project?"}]),
        }];

        let runtime_context = ContextAssembler::new(&workspace, &runtime).assemble_runtime(
            PromptMode::Execute,
            RuntimeContextInputs {
                cwd: "repo".to_string(),
                branch: "main".to_string(),
                session_id: "session-1".to_string(),
                history_len: history.len(),
                total_input_tokens: 0,
                total_output_tokens: 0,
                total_cache_hit_tokens: 0,
                total_cache_miss_tokens: 0,
                execution_mode: "execute".to_string(),
                plan_steps: Vec::new(),
                plan_explanation: None,
                todo_state: None,
                compact_state: crate::agent::CompactState {
                    context_window_tokens: Some(200_000),
                    compact_threshold_tokens: 190_000,
                    reserved_output_tokens: 1_024,
                    ..Default::default()
                },
                history: &history,
                vdb_uri: "memory://vdb",
                pending_interactions: Vec::new(),
                skill_listing: None,
                retrieved_memory_candidates: vec![RetrievedMemoryCandidate {
                    kind: RETRIEVED_WORKSPACE_MEMORY_KIND.to_string(),
                    label: "Memory: reference project path".to_string(),
                    detail: "content: Reference project source lives at /Users/example/reference-project."
                        .to_string(),
                    selection_reason: "retrieved as a candidate for the current turn query"
                        .to_string(),
                    rank: 1,
                }],
                file_search_candidates: vec![],
                tool_result_projection_policy: ToolResultProjectionPolicy::default(),
                tool_result_projection_report: ToolResultProjectionReport::default(),
            },
        );

        assert!(
            runtime_context
                .retrieval
                .memory_selection
                .selected_items
                .iter()
                .any(|item| item.kind == RETRIEVED_WORKSPACE_MEMORY_KIND)
        );
        assert!(runtime_context.assembly.entries.iter().any(|entry| {
            entry.layer == "active_memory_inputs"
                && entry.kind == RETRIEVED_WORKSPACE_MEMORY_KIND
                && entry.injected
        }));
    }

    #[test]
    fn assemble_runtime_includes_active_thread_working_set_in_memory_selection() {
        let workspace = test_workspace();
        let runtime = PromptRuntimeConfig::default();
        let history = vec![
            Message {
                role: "user".to_string(),
                content: json!([{"type":"text","text":"please continue the bootstrap cleanup"}]),
            },
            Message {
                role: "user".to_string(),
                content: json!([
                    {
                        "type": "tool_result",
                        "tool_use_id": "tool-shell-1",
                        "content": "diff preview"
                    }
                ]),
            },
        ];

        let runtime_context = ContextAssembler::new(&workspace, &runtime).assemble_runtime(
            PromptMode::Plan,
            RuntimeContextInputs {
                cwd: "repo".to_string(),
                branch: "main".to_string(),
                session_id: "session-1".to_string(),
                history_len: history.len(),
                total_input_tokens: 11,
                total_output_tokens: 7,
                total_cache_hit_tokens: 0,
                total_cache_miss_tokens: 0,
                execution_mode: "plan".to_string(),
                plan_steps: vec![(PlanStepStatus::Pending, "inspect bootstrap".to_string())],
                plan_explanation: Some("Keep one assembly path.".to_string()),
                todo_state: None,
                compact_state: crate::agent::CompactState {
                    context_window_tokens: Some(8_192),
                    compact_threshold_tokens: 7_000,
                    reserved_output_tokens: 1_024,
                    ..Default::default()
                },
                history: &history,
                vdb_uri: "",
                pending_interactions: vec![RuntimeInteractionInput {
                    kind: "approval".to_string(),
                    title: "Approve shell command".to_string(),
                    summary: "Allow one shell command in the repo root.".to_string(),
                    source: None,
                }],
                skill_listing: None,
                retrieved_memory_candidates: Vec::new(),
                file_search_candidates: vec![],
                tool_result_projection_policy: ToolResultProjectionPolicy::default(),
                tool_result_projection_report: ToolResultProjectionReport::default(),
            },
        );

        let selected_kinds = runtime_context
            .retrieval
            .memory_selection
            .selected_items
            .iter()
            .map(|item| item.kind.as_str())
            .collect::<Vec<_>>();
        assert!(selected_kinds.contains(&"plan_explanation"));
        assert!(selected_kinds.contains(&"plan_steps"));
        assert!(selected_kinds.contains(&"approval"));
        assert!(selected_kinds.contains(&"latest_user_request"));
        assert!(selected_kinds.contains(&"tool_result"));

        let active_memory_assembly_kinds = runtime_context
            .assembly
            .entries
            .iter()
            .filter(|entry| entry.layer == "active_memory_inputs")
            .map(|entry| entry.kind.as_str())
            .collect::<Vec<_>>();
        assert!(!active_memory_assembly_kinds.contains(&"plan_explanation"));
        assert!(!active_memory_assembly_kinds.contains(&"plan_steps"));
        assert!(!active_memory_assembly_kinds.contains(&"approval"));
        assert!(!active_memory_assembly_kinds.contains(&"latest_user_request"));
        assert!(!active_memory_assembly_kinds.contains(&"tool_result"));
    }

    #[test]
    fn runtime_budget_does_not_double_count_workspace_prompt_sources() {
        let state = crate::agent::CompactState {
            context_window_tokens: Some(3_000),
            compact_threshold_tokens: 2_800,
            reserved_output_tokens: 1_000,
            ..Default::default()
        };

        let budget = ContextBudgetView::from_compact_state(&state, 900, 900, 250, 200, 150, 75);

        assert_eq!(budget.workspace_prompt_budget, 250);
        assert_eq!(budget.remaining_input_budget, Some(675));
    }

    #[test]
    fn budget_for_passthrough_uses_backend_budget() {
        let workspace = test_workspace();
        let runtime = PromptRuntimeConfig::from_config(&RaraConfig::default());
        let budget = ContextBudget {
            context_window_tokens: 200_000,
            reserved_output_tokens: 4_096,
            compact_threshold_tokens: 190_000,
        };
        let backend = BudgetBackend {
            budget: Some(budget),
        };

        let result = ContextAssembler::new(&workspace, &runtime).budget_for(
            &backend,
            &[Message {
                role: "user".to_string(),
                content: json!([{"type":"text","text":"hello"}]),
            }],
            &[json!({"name":"read_file"})],
        );

        assert_eq!(result, Some(budget));
    }
}
