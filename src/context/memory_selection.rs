use std::collections::BTreeMap;

use crate::agent::{Message, PlanStepStatus};
use crate::context::assembler::{
    RuntimeInteractionInput, estimate_text_tokens, latest_tool_results, latest_user_request,
};
use crate::context::{
    CompactionSourceContextEntry, DropReason, MemorySelectionContextView,
    MemorySelectionItemContextEntry, RetrievalCandidate, RetrievedMemoryRenderItem,
    is_retrieved_memory_kind, render_retrieved_memory_context,
};
use crate::prompt::PromptSource;

pub(crate) fn memory_selection(
    prompt_sources: &[PromptSource],
    plan_explanation: Option<&str>,
    plan_steps: &[(PlanStepStatus, String)],
    pending_interactions: &[RuntimeInteractionInput],
    compacted_history: &[CompactionSourceContextEntry],
    history: &[Message],
    retrieval_candidates: &[RetrievalCandidate],
    selection_budget_tokens: Option<usize>,
) -> MemorySelectionContextView {
    let mut selected_items = fixed_memory_selection_items(
        prompt_sources,
        plan_explanation,
        plan_steps,
        pending_interactions,
        compacted_history,
        history,
    );
    let fixed_kinds = selected_items
        .iter()
        .map(|item| item.kind.clone())
        .collect::<Vec<_>>();
    let mut discretionary = select_memory_candidates(
        retrieval_candidates
            .iter()
            .cloned()
            .map(memory_selection_candidate_from_retrieval_candidate)
            .collect(),
        selection_budget_tokens,
        fixed_kinds.as_slice(),
    );
    selected_items.append(&mut discretionary.selected_items);
    for (idx, item) in selected_items.iter_mut().enumerate() {
        item.order = idx + 1;
    }

    let mut available_items = discretionary.available_items;
    if !selected_items
        .iter()
        .any(|item| item.kind == "workspace_memory")
    {
        available_items.push(workspace_memory_available_item(prompt_sources));
    }
    for (idx, item) in available_items.iter_mut().enumerate() {
        item.order = idx + 1;
    }

    let mut dropped_items = discretionary.dropped_items;
    for (idx, item) in dropped_items.iter_mut().enumerate() {
        item.order = idx + 1;
    }

    MemorySelectionContextView {
        selection_budget_tokens,
        selected_items,
        available_items,
        dropped_items,
    }
}

#[derive(Debug, Clone)]
struct MemorySelectionCandidate {
    kind: String,
    label: String,
    detail: String,
    selection_reason: String,
    budget_impact_tokens: Option<usize>,
    priority: usize,
    dedupe_key: Option<String>,
    selectable: bool,
    dropped_reason: DropReason,
}

#[derive(Debug, Default)]
struct MemorySelectionDecision {
    selected_items: Vec<MemorySelectionItemContextEntry>,
    available_items: Vec<MemorySelectionItemContextEntry>,
    dropped_items: Vec<MemorySelectionItemContextEntry>,
}

fn fixed_memory_selection_items(
    prompt_sources: &[PromptSource],
    plan_explanation: Option<&str>,
    plan_steps: &[(PlanStepStatus, String)],
    pending_interactions: &[RuntimeInteractionInput],
    compacted_history: &[CompactionSourceContextEntry],
    history: &[Message],
) -> Vec<MemorySelectionItemContextEntry> {
    let mut items = Vec::new();
    items.extend(workspace_memory_selected_items(prompt_sources));
    items.extend(compacted_history_selected_items(compacted_history));
    items.extend(active_thread_selected_items(
        plan_explanation,
        plan_steps,
        pending_interactions,
        history,
    ));
    items
}

fn active_thread_selected_items(
    plan_explanation: Option<&str>,
    plan_steps: &[(PlanStepStatus, String)],
    pending_interactions: &[RuntimeInteractionInput],
    history: &[Message],
) -> Vec<MemorySelectionItemContextEntry> {
    let mut items = Vec::new();

    if let Some(plan_explanation) = plan_explanation.filter(|value| !value.trim().is_empty()) {
        items.push(MemorySelectionItemContextEntry {
            order: 0,
            kind: "plan_explanation".to_string(),
            label: "Plan Explanation".to_string(),
            detail: plan_explanation.trim().to_string(),
            selection_reason: "selected because the active thread currently carries a structured plan explanation that must remain visible to the runtime and restore surfaces".to_string(),
            budget_impact_tokens: Some(estimate_text_tokens(plan_explanation)),
            dropped_reason: None,
        });
    }

    if !plan_steps.is_empty() {
        let detail = plan_steps
            .iter()
            .map(|(status, step)| {
                let status = match status {
                    PlanStepStatus::Pending => "pending",
                    PlanStepStatus::InProgress => "in_progress",
                    PlanStepStatus::Completed => "completed",
                };
                format!("[{status}] {step}")
            })
            .collect::<Vec<_>>()
            .join("\n");
        items.push(MemorySelectionItemContextEntry {
            order: 0,
            kind: "plan_steps".to_string(),
            label: "Plan Steps".to_string(),
            detail: detail.clone(),
            selection_reason: "selected because structured plan steps are part of the current thread working set and must survive restore".to_string(),
            budget_impact_tokens: Some(estimate_text_tokens(detail.as_str())),
            dropped_reason: None,
        });
    }

    for interaction in pending_interactions {
        items.push(MemorySelectionItemContextEntry {
            order: 0,
            kind: interaction.kind.clone(),
            label: interaction.title.clone(),
            detail: interaction.summary.clone(),
            selection_reason: "selected because pending interactions are active runtime obligations that must remain available until answered".to_string(),
            budget_impact_tokens: Some(
                estimate_text_tokens(interaction.title.as_str())
                    + estimate_text_tokens(interaction.summary.as_str()),
            ),
            dropped_reason: None,
        });
    }

    if let Some(user_request) = latest_user_request(history) {
        items.push(MemorySelectionItemContextEntry {
            order: 0,
            kind: "latest_user_request".to_string(),
            label: "Latest User Request".to_string(),
            detail: user_request.clone(),
            selection_reason: "selected because the latest user request anchors the current turn objective and should stay in the active working set".to_string(),
            budget_impact_tokens: Some(estimate_text_tokens(user_request.as_str())),
            dropped_reason: None,
        });
    }

    for (label, detail) in latest_tool_results(history) {
        items.push(MemorySelectionItemContextEntry {
            order: 0,
            kind: "tool_result".to_string(),
            label,
            detail: detail.clone(),
            selection_reason: "selected because recent tool results are part of the active thread working set until the assistant synthesizes a final answer".to_string(),
            budget_impact_tokens: Some(estimate_text_tokens(detail.as_str())),
            dropped_reason: None,
        });
    }

    items
}

fn workspace_memory_selected_items(
    prompt_sources: &[PromptSource],
) -> Vec<MemorySelectionItemContextEntry> {
    prompt_sources
        .iter()
        .filter(|source| source.kind_label() == "local_memory")
        .map(workspace_memory_selected_item)
        .collect()
}

fn workspace_memory_selected_item(source: &PromptSource) -> MemorySelectionItemContextEntry {
    MemorySelectionItemContextEntry {
        order: 0,
        kind: "workspace_memory".to_string(),
        label: "Workspace Memory".to_string(),
        detail: format!(
            "{}; {}",
            source.display_path,
            summarize_workspace_memory_source(source.content.as_str())
        ),
        selection_reason: "selected because the current effective prompt includes the workspace memory file as an active input".to_string(),
        budget_impact_tokens: Some(estimate_text_tokens(source.content.as_str())),
        dropped_reason: None,
    }
}

fn compacted_history_selected_items(
    entries: &[CompactionSourceContextEntry],
) -> Vec<MemorySelectionItemContextEntry> {
    entries
        .iter()
        .filter(|entry| entry.kind != "compact_boundary")
        .map(|entry| MemorySelectionItemContextEntry {
            order: 0,
            kind: entry.kind.clone(),
            label: entry.label.clone(),
            budget_impact_tokens: Some(estimate_text_tokens(entry.detail.as_str())),
            detail: entry.detail.clone(),
            selection_reason: entry.inclusion_reason.clone(),
            dropped_reason: None,
        })
        .collect()
}

fn memory_selection_candidate_from_retrieval_candidate(
    candidate: RetrievalCandidate,
) -> MemorySelectionCandidate {
    MemorySelectionCandidate {
        kind: candidate.kind,
        label: candidate.label,
        detail: candidate.detail,
        selection_reason: candidate.selection_reason,
        budget_impact_tokens: candidate.budget_impact_tokens,
        priority: candidate.priority,
        dedupe_key: candidate.dedupe_key,
        selectable: candidate.selectable,
        dropped_reason: DropReason::NotSelected {
            reason: candidate.not_selected_reason,
        },
    }
}

fn select_memory_candidates(
    mut candidates: Vec<MemorySelectionCandidate>,
    selection_budget_tokens: Option<usize>,
    fixed_selected_kinds: &[String],
) -> MemorySelectionDecision {
    candidates.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| left.dedupe_key.cmp(&right.dedupe_key))
            .then_with(|| left.kind.cmp(&right.kind))
            .then_with(|| left.label.cmp(&right.label))
            .then_with(|| left.detail.cmp(&right.detail))
    });
    let mut remaining_budget = selection_budget_tokens;
    let has_compacted_history = fixed_selected_kinds
        .iter()
        .any(|kind| is_compacted_history_kind(kind.as_str()));
    let mut decision = MemorySelectionDecision::default();
    let mut selected_kinds = fixed_selected_kinds.to_vec();
    let mut selected_retrieved_items: Vec<(String, String)> = Vec::new();
    let mut selected_dedupe_keys = BTreeMap::<String, String>::new();

    for candidate in candidates {
        let candidate_budget_impact =
            candidate_budget_impact_tokens(&candidate, selected_retrieved_items.as_slice());
        let is_retrieved_memory = is_retrieved_memory_kind(candidate.kind.as_str());
        let selected_retrieved_item =
            is_retrieved_memory.then(|| (candidate.label.clone(), candidate.detail.clone()));
        let should_drop: Option<DropReason> = if !candidate.selectable {
            Some(candidate.dropped_reason.clone())
        } else if let Some((dedupe_key, winning_label)) = candidate
            .dedupe_key
            .as_ref()
            .and_then(|key| selected_dedupe_keys.get_key_value(key))
        {
            Some(DropReason::NotSelected {
                reason: format!(
                    "deduped_by={winning_label} because candidate dedupe_key={dedupe_key} already won selection"
                ),
            })
        } else if candidate.kind == "thread_history" && has_compacted_history {
            Some(DropReason::NotSelected {
                reason: "not selected because compacted thread history already provides a more focused carried-over thread view".to_string(),
            })
        } else if selected_kinds
            .iter()
            .any(|kind| kind == "retrieved_thread_context" && candidate.kind == "thread_history")
        {
            Some(DropReason::NotSelected {
                reason:
                    "not selected because a more focused retrieved thread-context candidate already won the current memory selection".to_string(),
            })
        } else if let (Some(remaining), Some(cost)) = (remaining_budget, candidate_budget_impact) {
            (cost > remaining).then(|| DropReason::BudgetExceeded {
                reason: format!(
                    "not selected because it would exceed the remaining memory-selection budget ({cost} > {remaining})"
                ),
            })
        } else {
            None
        };

        if let Some(dropped_reason) = should_drop {
            let is_budget = matches!(&dropped_reason, DropReason::BudgetExceeded { .. });
            let item = MemorySelectionItemContextEntry {
                order: 0,
                kind: candidate.kind,
                label: candidate.label,
                detail: candidate.detail,
                selection_reason: candidate.selection_reason,
                budget_impact_tokens: candidate_budget_impact,
                dropped_reason: Some(dropped_reason),
            };
            if is_budget {
                decision.dropped_items.push(item);
            } else {
                decision.available_items.push(item);
            }
            continue;
        }

        if let (Some(remaining), Some(cost)) = (remaining_budget.as_mut(), candidate_budget_impact)
        {
            *remaining = remaining.saturating_sub(cost);
        }
        if let Some(item) = selected_retrieved_item {
            selected_retrieved_items.push(item);
        }
        if let Some(dedupe_key) = candidate.dedupe_key.as_ref() {
            selected_dedupe_keys.insert(dedupe_key.clone(), candidate.label.clone());
        }
        selected_kinds.push(candidate.kind.clone());
        decision
            .selected_items
            .push(MemorySelectionItemContextEntry {
                order: 0,
                kind: candidate.kind,
                label: candidate.label,
                detail: candidate.detail,
                selection_reason: candidate.selection_reason,
                budget_impact_tokens: candidate_budget_impact,
                dropped_reason: None,
            });
    }

    decision
}

fn is_compacted_history_kind(kind: &str) -> bool {
    matches!(
        kind,
        "compacted_summary" | "recent_files" | "recent_file_excerpts"
    ) || kind.starts_with("compacted_")
}

fn candidate_budget_impact_tokens(
    candidate: &MemorySelectionCandidate,
    selected_retrieved_items: &[(String, String)],
) -> Option<usize> {
    if !is_retrieved_memory_kind(candidate.kind.as_str()) {
        return candidate.budget_impact_tokens;
    }
    Some(retrieved_memory_incremental_budget_impact(
        selected_retrieved_items,
        candidate,
    ))
}

fn retrieved_memory_incremental_budget_impact(
    selected_items: &[(String, String)],
    candidate: &MemorySelectionCandidate,
) -> usize {
    let current = retrieved_memory_context_budget(selected_items);
    let mut with_candidate = selected_items.to_vec();
    with_candidate.push((candidate.label.clone(), candidate.detail.clone()));
    retrieved_memory_context_budget(with_candidate.as_slice()).saturating_sub(current)
}

fn retrieved_memory_context_budget(items: &[(String, String)]) -> usize {
    let render_items = items
        .iter()
        .map(|(label, detail)| RetrievedMemoryRenderItem {
            label: label.as_str(),
            detail: detail.as_str(),
        })
        .collect::<Vec<_>>();
    render_retrieved_memory_context(render_items.as_slice())
        .map(|rendered| estimate_text_tokens(&rendered))
        .unwrap_or_default()
}

fn workspace_memory_available_item(
    prompt_sources: &[PromptSource],
) -> MemorySelectionItemContextEntry {
    let workspace_memory_available = prompt_sources
        .iter()
        .any(|source| source.kind_label() == "local_memory");
    MemorySelectionItemContextEntry {
        order: 0,
        kind: "workspace_memory".to_string(),
        label: "Workspace Memory".to_string(),
        detail: if workspace_memory_available {
            "workspace prompt source is available, but not active in the current assembled prompt".to_string()
        } else {
            "no injected workspace memory prompt source".to_string()
        },
        selection_reason: "workspace memory participates in the selection contract even when it is not part of the current assembled working set".to_string(),
        budget_impact_tokens: None,
        dropped_reason: Some(DropReason::NotSelected { reason: if workspace_memory_available {
            "available for recall, but not selected into the current turn because workspace memory was not activated as a prompt input".to_string()
        } else {
            "no workspace memory candidate is currently available".to_string()
        }}),
    }
}

fn summarize_workspace_memory_source(content: &str) -> String {
    let line_count = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    match line_count {
        0 => "empty".to_string(),
        1 => "1 non-empty line".to_string(),
        _ => format!("{line_count} non-empty lines"),
    }
}

#[cfg(test)]
mod tests {
    include!("memory_selection_tests.rs");
}
