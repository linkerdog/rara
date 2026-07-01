use crate::agent::{Message, PlanStepStatus};
use crate::context::assembler::{
    RuntimeInteractionInput, estimate_text_tokens, latest_tool_results, latest_user_request,
};
use crate::context::{
    CompactionSourceContextEntry, ContextAssemblyEntry, ContextAssemblyView,
    MemorySelectionItemContextEntry, is_retrieved_memory_kind,
};
use crate::prompt::EffectivePrompt;

#[allow(clippy::too_many_arguments)]
// Context assembly is the reporting boundary that combines already-built
// runtime views; a parameter struct would duplicate the caller's context bag.
pub(crate) fn assemble_context_view(
    effective_prompt: &EffectivePrompt,
    plan_explanation: Option<&str>,
    plan_steps: &[(PlanStepStatus, String)],
    pending_interactions: &[RuntimeInteractionInput],
    compaction_entries: &[CompactionSourceContextEntry],
    selected_memory_items: &[MemorySelectionItemContextEntry],
    available_memory_items: &[MemorySelectionItemContextEntry],
    dropped_memory_items: &[MemorySelectionItemContextEntry],
    history: &[Message],
    skill_listing: Option<&str>,
) -> ContextAssemblyView {
    let mut entries = Vec::new();

    let mut push = |mut entry: ContextAssemblyEntry| {
        entry.order = entries.len() + 1;
        entries.push(entry);
    };

    push(ContextAssemblyEntry {
        order: 0,
        cache_status: None,
        layer: "stable_instructions".to_string(),
        kind: format!("base_prompt:{}", effective_prompt.base_prompt_kind.label()),
        label: "Base System Prompt".to_string(),
        source_path: None,
        injected: true,
        inclusion_reason: "included as the stable system prompt scaffold for every turn"
            .to_string(),
        budget_impact_tokens: Some(estimate_text_tokens(effective_prompt.text.as_str())),
        dropped_reason: None,
    });

    for source in &effective_prompt.sources {
        let kind = source.kind_label().to_string();
        let layer = match kind.as_str() {
            "user_instruction" => "stable_instructions",
            "project_instruction" => "stable_instructions",
            "local_memory" => "workspace_prompt_sources",
            _ => "workspace_prompt_sources",
        };
        push(ContextAssemblyEntry {
            order: 0,
            cache_status: None,
            layer: layer.to_string(),
            kind,
            label: source.label.clone(),
            source_path: Some(source.display_path.clone()),
            injected: true,
            inclusion_reason: source.inclusion_reason().to_string(),
            budget_impact_tokens: Some(estimate_text_tokens(source.content.as_str())),
            dropped_reason: None,
        });
    }

    if let Some(plan_explanation) = plan_explanation.filter(|value| !value.trim().is_empty()) {
        push(ContextAssemblyEntry {
            order: 0,
            cache_status: None,
            layer: "active_turn_state".to_string(),
            kind: "plan_explanation".to_string(),
            label: "Plan Explanation".to_string(),
            source_path: None,
            injected: true,
            inclusion_reason:
                "included because the active thread currently carries a structured plan explanation"
                    .to_string(),
            budget_impact_tokens: Some(estimate_text_tokens(plan_explanation)),
            dropped_reason: None,
        });
    }

    if !plan_steps.is_empty() {
        let detail = plan_steps
            .iter()
            .map(|(_, step)| step.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        push(ContextAssemblyEntry {
            order: 0,
            cache_status: None,
            layer: "active_turn_state".to_string(),
            kind: "plan_steps".to_string(),
            label: "Plan Steps".to_string(),
            source_path: None,
            injected: true,
            inclusion_reason:
                "included because structured plan steps are part of the current active thread state"
                    .to_string(),
            budget_impact_tokens: Some(estimate_text_tokens(detail.as_str())),
            dropped_reason: None,
        });
    }

    for interaction in pending_interactions {
        push(ContextAssemblyEntry {
            order: 0,
            cache_status: None,
            layer: "active_turn_state".to_string(),
            kind: interaction.kind.clone(),
            label: interaction.title.clone(),
            source_path: interaction.source.clone(),
            injected: true,
            inclusion_reason:
                "included because pending interactions are active runtime obligations".to_string(),
            budget_impact_tokens: Some(
                estimate_text_tokens(interaction.title.as_str())
                    + estimate_text_tokens(interaction.summary.as_str()),
            ),
            dropped_reason: None,
        });
    }

    if let Some(user_request) = latest_user_request(history) {
        push(ContextAssemblyEntry {
            order: 0,
            cache_status: None,
            layer: "active_turn_state".to_string(),
            kind: "latest_user_request".to_string(),
            label: "Latest User Request".to_string(),
            source_path: None,
            injected: true,
            inclusion_reason:
                "included because the latest user request anchors the current turn objective"
                    .to_string(),
            budget_impact_tokens: Some(estimate_text_tokens(user_request.as_str())),
            dropped_reason: None,
        });
    }

    for (label, detail) in latest_tool_results(history) {
        push(ContextAssemblyEntry {
            order: 0,
            cache_status: None,
            layer: "active_turn_state".to_string(),
            kind: "tool_result".to_string(),
            label,
            source_path: None,
            injected: true,
            inclusion_reason:
                "included because recent tool results are part of the current working set for the active turn"
                    .to_string(),
            budget_impact_tokens: Some(estimate_text_tokens(detail.as_str())),
            dropped_reason: None,
        });
    }

    for entry in compaction_entries {
        push(ContextAssemblyEntry {
            order: 0,
            cache_status: None,
            layer: "compacted_history".to_string(),
            kind: entry.kind.clone(),
            label: entry.label.clone(),
            source_path: Some(entry.source_descriptor.clone()),
            injected: true,
            inclusion_reason: entry.inclusion_reason.clone(),
            budget_impact_tokens: Some(estimate_text_tokens(entry.detail.as_str())),
            dropped_reason: None,
        });
    }

    for item in selected_memory_items
        .iter()
        .filter(|item| is_retrieved_memory_kind(item.kind.as_str()))
    {
        push(ContextAssemblyEntry {
            order: 0,
            cache_status: None,
            layer: "active_memory_inputs".to_string(),
            kind: item.kind.clone(),
            label: item.label.clone(),
            source_path: None,
            injected: true,
            inclusion_reason: item.selection_reason.clone(),
            budget_impact_tokens: item.budget_impact_tokens,
            dropped_reason: None,
        });
    }

    for item in available_memory_items {
        push(ContextAssemblyEntry {
            order: 0,
            cache_status: None,
            layer: "retrieval_ready".to_string(),
            kind: item.kind.clone(),
            label: item.label.clone(),
            source_path: None,
            injected: false,
            inclusion_reason: item.selection_reason.clone(),
            budget_impact_tokens: item.budget_impact_tokens,
            dropped_reason: item.dropped_reason.as_ref().map(|r| r.reason().to_string()),
        });
    }

    for item in dropped_memory_items {
        push(ContextAssemblyEntry {
            order: 0,
            cache_status: None,
            layer: "retrieval_ready".to_string(),
            kind: item.kind.clone(),
            label: item.label.clone(),
            source_path: None,
            injected: false,
            inclusion_reason: item.selection_reason.clone(),
            budget_impact_tokens: item.budget_impact_tokens,
            dropped_reason: item.dropped_reason.as_ref().map(|r| r.reason().to_string()),
        });
    }

    if let Some(skill_text) = skill_listing.filter(|v| !v.trim().is_empty()) {
        push(ContextAssemblyEntry {
            order: 0,
            cache_status: None,
            layer: "skill_listing".to_string(),
            kind: "skill_listing".to_string(),
            label: "Available Skills".to_string(),
            source_path: None,
            injected: true,
            inclusion_reason: "injected as a per-turn skill_listing attachment for agent discovery"
                .to_string(),
            budget_impact_tokens: Some(estimate_text_tokens(skill_text)),
            dropped_reason: None,
        });
    }

    ContextAssemblyView { entries }
}
