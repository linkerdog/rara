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
    use serde_json::json;

    use super::*;
    use crate::context::retrieval_provider::retrieval_candidate_from_retrieved_memory;
    use crate::context::{
        RETRIEVED_THREAD_CONTEXT_KIND, RETRIEVED_WORKSPACE_MEMORY_KIND, RetrievalRequest,
        RetrievedMemoryCandidate, retrieval_candidates,
    };

    #[allow(clippy::too_many_arguments)]
    fn memory_selection_for_test(
        prompt_sources: &[PromptSource],
        plan_explanation: Option<&str>,
        plan_steps: &[(PlanStepStatus, String)],
        pending_interactions: &[RuntimeInteractionInput],
        compacted_history: &[CompactionSourceContextEntry],
        history: &[Message],
        session_id: &str,
        vdb_uri: &str,
        retrieved_memory_candidates: &[RetrievedMemoryCandidate],
        file_search_candidates: &[RetrievalCandidate],
        selection_budget_tokens: Option<usize>,
    ) -> MemorySelectionContextView {
        let query = latest_user_request(history).unwrap_or_default();
        let request = RetrievalRequest {
            query: query.as_str(),
            session_id,
            history,
            vdb_uri,
        };
        let candidates = retrieval_candidates(
            &request,
            retrieved_memory_candidates,
            file_search_candidates,
        );
        memory_selection(
            prompt_sources,
            plan_explanation,
            plan_steps,
            pending_interactions,
            compacted_history,
            history,
            candidates.as_slice(),
            selection_budget_tokens,
        )
    }

    // ── Non-vector selection path ──────────────────────────────────────────

    #[test]
    fn thread_history_selected_when_no_compacted_history_and_budget_allows() {
        let history = vec![
            Message {
                role: "user".to_string(),
                content: json!("hello"),
            },
            Message {
                role: "assistant".to_string(),
                content: json!("hi there"),
            },
        ];
        let result = memory_selection_for_test(
            &[],
            None,
            &[],
            &[],
            &[], // no compacted history
            &history,
            "session-1",
            "",
            &[],
            &[],
            Some(10_000),
        );

        let selected_kinds: Vec<&str> = result
            .selected_items
            .iter()
            .map(|item| item.kind.as_str())
            .collect();
        assert!(
            selected_kinds.contains(&"latest_user_request"),
            "latest user request should be a fixed selected item"
        );
        assert!(
            selected_kinds.contains(&"thread_history"),
            "thread_history should be selected when no compacted history exists and budget allows"
        );
        assert!(result.dropped_items.is_empty());
    }

    #[test]
    fn thread_history_available_not_selected_when_compacted_history_exists() {
        let history = vec![Message {
            role: "user".to_string(),
            content: json!("hello"),
        }];
        let compacted = vec![CompactionSourceContextEntry {
            order: 1,
            kind: "compacted_summary".to_string(),
            label: "Compacted Summary".to_string(),
            source_descriptor: "history.compaction.summary".to_string(),
            detail: "previous work".to_string(),
            inclusion_reason: "carried forward".to_string(),
        }];
        let result = memory_selection_for_test(
            &[],
            None,
            &[],
            &[],
            &compacted,
            &history,
            "session-1",
            "",
            &[],
            &[],
            Some(10_000),
        );

        let available_kinds: Vec<&str> = result
            .available_items
            .iter()
            .map(|item| item.kind.as_str())
            .collect();
        assert!(
            available_kinds.contains(&"thread_history"),
            "thread_history should be available but not selected when compacted history already covers it"
        );
        let selected_kinds: Vec<&str> = result
            .selected_items
            .iter()
            .map(|item| item.kind.as_str())
            .collect();
        assert!(
            !selected_kinds.contains(&"thread_history"),
            "thread_history should not be selected when compacted history exists"
        );
    }

    #[test]
    fn thread_history_available_not_selected_when_generic_compacted_carry_over_exists() {
        let history = vec![Message {
            role: "user".to_string(),
            content: json!("hello"),
        }];
        let compacted = vec![CompactionSourceContextEntry {
            order: 1,
            kind: "compacted_memory".to_string(),
            label: "Memory Carry-over".to_string(),
            source_descriptor: "history.compaction.memory".to_string(),
            detail: "stable transcript path".to_string(),
            inclusion_reason: "carried forward".to_string(),
        }];
        let result = memory_selection_for_test(
            &[],
            None,
            &[],
            &[],
            &compacted,
            &history,
            "session-1",
            "",
            &[],
            &[],
            Some(10_000),
        );

        assert!(result.selected_items.iter().any(|item| {
            item.kind == "compacted_memory"
                && item.selection_reason == "carried forward"
                && item.detail == "stable transcript path"
        }));
        assert!(
            result
                .available_items
                .iter()
                .any(|item| item.kind == "thread_history")
        );
        assert!(
            !result
                .selected_items
                .iter()
                .any(|item| item.kind == "thread_history")
        );
    }

    #[test]
    fn vector_memory_is_available_but_not_selectable() {
        let history: Vec<Message> = vec![];
        let result = memory_selection_for_test(
            &[],
            None,
            &[],
            &[],
            &[],
            &history,
            "session-1",
            "memory://vdb",
            &[],
            &[],
            Some(10_000),
        );

        let available_kinds: Vec<&str> = result
            .available_items
            .iter()
            .map(|item| item.kind.as_str())
            .collect();
        assert!(
            available_kinds.contains(&"vector_memory"),
            "vector_memory should appear in available when a vdb URI is configured"
        );
        let vector_entry = result
            .available_items
            .iter()
            .find(|item| item.kind == "vector_memory")
            .expect("vector_memory should be present");
        assert!(
            vector_entry
                .dropped_reason
                .as_ref()
                .is_some_and(|r| r.reason().contains("not implemented")),
            "vector_memory should explain it is not implemented yet"
        );
    }

    #[test]
    fn retrieval_tool_results_from_history_are_captured_as_candidates() {
        let history = vec![
            Message {
                role: "assistant".to_string(),
                content: json!([
                    {
                        "type": "tool_use",
                        "id": "tool-retrieve-1",
                        "name": "retrieve_experience",
                        "input": { "query": "bootstrap contract" }
                    }
                ]),
            },
            Message {
                role: "user".to_string(),
                content: json!([
                    {
                        "type": "tool_result",
                        "tool_use_id": "tool-retrieve-1",
                        "content": "Tool retrieve_experience completed.\nPayload:\n{\n  \"relevant_experiences\": [\"Use shared bootstrap.\"]\n}"
                    }
                ]),
            },
        ];
        // Budget of 1 token forces the retrieval candidate to be dropped,
        // proving it was captured as a candidate.
        let result = memory_selection_for_test(
            &[],
            None,
            &[],
            &[],
            &[],
            &history,
            "session-1",
            "",
            &[],
            &[],
            Some(1),
        );

        let dropped_kinds: Vec<&str> = result
            .dropped_items
            .iter()
            .map(|item| item.kind.as_str())
            .collect();
        let selected_kinds: Vec<&str> = result
            .selected_items
            .iter()
            .map(|item| item.kind.as_str())
            .collect();
        let available_kinds: Vec<&str> = result
            .available_items
            .iter()
            .map(|item| item.kind.as_str())
            .collect();
        // The retrieval candidate should appear in one of the three lists,
        // proving it was captured from history.
        let all_kinds: Vec<&&str> = dropped_kinds
            .iter()
            .chain(selected_kinds.iter())
            .chain(available_kinds.iter())
            .collect();
        assert!(
            all_kinds.contains(&&"retrieved_workspace_memory"),
            "retrieval tool candidate from history must appear in selected, available, or dropped"
        );
    }

    #[test]
    fn retrieval_tool_results_selected_when_budget_allows() {
        let history = vec![
            Message {
                role: "assistant".to_string(),
                content: json!([
                    {
                        "type": "tool_use",
                        "id": "tool-retrieve-1",
                        "name": "retrieve_session_context",
                        "input": { "query": "auth flow" }
                    }
                ]),
            },
            Message {
                role: "user".to_string(),
                content: json!([
                    {
                        "type": "tool_result",
                        "tool_use_id": "tool-retrieve-1",
                        "content": "Tool retrieve_session_context completed.\nPayload:\n{\n  \"status\": \"ok\",\n  \"summary\": \"Auth picker moved.\"\n}"
                    }
                ]),
            },
        ];
        let result = memory_selection_for_test(
            &[],
            None,
            &[],
            &[],
            &[],
            &history,
            "session-1",
            "",
            &[],
            &[],
            Some(10_000),
        );

        let selected_kinds: Vec<&str> = result
            .selected_items
            .iter()
            .map(|item| item.kind.as_str())
            .collect();
        assert!(
            selected_kinds.contains(&"retrieved_thread_context"),
            "retrieve_session_context results should be selected when budget allows"
        );
    }

    #[test]
    fn direct_retrieved_memory_candidates_are_selected_when_budget_allows() {
        let history = vec![Message {
            role: "user".to_string(),
            content: json!([{"type":"text","text":"Where is the reference project?"}]),
        }];
        let retrieved = vec![RetrievedMemoryCandidate {
            kind: RETRIEVED_WORKSPACE_MEMORY_KIND.to_string(),
            label: "Memory: reference project path".to_string(),
            detail: "content: Reference project source lives at /Users/example/reference-project."
                .to_string(),
            selection_reason: "retrieved as a candidate for the current turn query".to_string(),
            rank: 1,
        }];

        let result = memory_selection_for_test(
            &[],
            None,
            &[],
            &[],
            &[],
            &history,
            "session-1",
            "memory://vdb",
            &retrieved,
            &[],
            Some(10_000),
        );

        let selected = result
            .selected_items
            .iter()
            .find(|item| item.kind == RETRIEVED_WORKSPACE_MEMORY_KIND)
            .expect("direct retrieved workspace memory should be selected");
        assert_eq!(selected.label, "Memory: reference project path");
        assert!(selected.detail.contains("/Users/example/reference-project"));
        let expected_rendered = render_retrieved_memory_context(&[RetrievedMemoryRenderItem {
            label: selected.label.as_str(),
            detail: selected.detail.as_str(),
        }])
        .expect("retrieved memory context should render");
        assert_eq!(
            selected.budget_impact_tokens,
            Some(estimate_text_tokens(&expected_rendered))
        );
    }

    #[test]
    fn retrieved_memory_adapter_builds_typed_candidate_boundary() {
        let candidate = RetrievedMemoryCandidate {
            kind: RETRIEVED_WORKSPACE_MEMORY_KIND.to_string(),
            label: "Memory: Reference Project Path".to_string(),
            detail: "content: Reference project source lives at /Users/example/reference-project."
                .to_string(),
            selection_reason: "retrieved as a candidate for the current turn query".to_string(),
            rank: 3,
        };

        let typed = retrieval_candidate_from_retrieved_memory(&candidate);

        assert_eq!(typed.source.source_type, "memory_record");
        assert_eq!(typed.scope, "workspace");
        assert_eq!(typed.priority, 23);
        assert_eq!(typed.rank, 3);
        assert_eq!(
            typed.dedupe_key.as_deref(),
            Some(
                "memory_record:retrieved_workspace_memory:content-reference-project-source-lives-at-users-example"
            )
        );
        assert!(typed.budget_impact_tokens.is_some());
        assert!(typed.availability_reason.contains("retrieval provider"));
    }

    #[test]
    fn typed_retrieval_candidate_priority_drives_selection_order() {
        let history = vec![Message {
            role: "user".to_string(),
            content: json!([{"type":"text","text":"What happened in the prior session?"}]),
        }];
        let retrieved = vec![
            RetrievedMemoryCandidate {
                kind: RETRIEVED_WORKSPACE_MEMORY_KIND.to_string(),
                label: "Memory: workspace fact".to_string(),
                detail: "content: workspace fact".to_string(),
                selection_reason: "retrieved workspace memory".to_string(),
                rank: 1,
            },
            RetrievedMemoryCandidate {
                kind: RETRIEVED_THREAD_CONTEXT_KIND.to_string(),
                label: "Session Context session-1#2".to_string(),
                detail: "content: focused session context".to_string(),
                selection_reason: "retrieved session context".to_string(),
                rank: 1,
            },
        ];

        let result = memory_selection_for_test(
            &[],
            None,
            &[],
            &[],
            &[],
            &history,
            "session-1",
            "memory://vdb",
            &retrieved,
            &[],
            Some(10_000),
        );

        let selected_retrieval = result
            .selected_items
            .iter()
            .filter(|item| {
                matches!(
                    item.kind.as_str(),
                    RETRIEVED_THREAD_CONTEXT_KIND | RETRIEVED_WORKSPACE_MEMORY_KIND
                )
            })
            .map(|item| item.kind.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            selected_retrieval,
            vec![
                RETRIEVED_THREAD_CONTEXT_KIND,
                RETRIEVED_WORKSPACE_MEMORY_KIND
            ],
            "typed priorities should preserve focused thread context before workspace memory"
        );
    }

    #[test]
    fn retrieved_memory_dedupe_key_keeps_first_winner_and_reports_loser() {
        let history = vec![Message {
            role: "user".to_string(),
            content: json!([{"type":"text","text":"Where is the reference project?"}]),
        }];
        let retrieved = vec![
            RetrievedMemoryCandidate {
                kind: RETRIEVED_WORKSPACE_MEMORY_KIND.to_string(),
                label: "Memory: reference project path".to_string(),
                detail:
                    "content: Reference project source lives at /Users/example/reference-project."
                        .to_string(),
                selection_reason: "retrieved as a candidate for the current turn query".to_string(),
                rank: 1,
            },
            RetrievedMemoryCandidate {
                kind: RETRIEVED_WORKSPACE_MEMORY_KIND.to_string(),
                label: "Memory: duplicate reference project path".to_string(),
                detail:
                    "content: Reference project source lives at /Users/example/reference-project."
                        .to_string(),
                selection_reason: "retrieved as a candidate for the current turn query".to_string(),
                rank: 2,
            },
        ];

        let result = memory_selection_for_test(
            &[],
            None,
            &[],
            &[],
            &[],
            &history,
            "session-1",
            "memory://vdb",
            &retrieved,
            &[],
            Some(10_000),
        );

        let selected_retrieved = result
            .selected_items
            .iter()
            .filter(|item| item.kind == RETRIEVED_WORKSPACE_MEMORY_KIND)
            .collect::<Vec<_>>();
        assert_eq!(selected_retrieved.len(), 1);
        assert_eq!(
            selected_retrieved[0].label,
            "Memory: reference project path"
        );

        let duplicate = result
            .available_items
            .iter()
            .find(|item| item.label == "Memory: duplicate reference project path")
            .expect("duplicate should remain observable as available");
        assert!(
            duplicate
                .dropped_reason
                .as_ref()
                .expect("dedupe reason")
                .reason()
                .contains("deduped_by=Memory: reference project path")
        );
    }

    #[test]
    fn direct_retrieved_memory_candidates_charge_shared_context_overhead_once() {
        let history = vec![Message {
            role: "user".to_string(),
            content: json!([{"type":"text","text":"Where are the reference project notes?"}]),
        }];
        let retrieved = vec![
            RetrievedMemoryCandidate {
                kind: RETRIEVED_WORKSPACE_MEMORY_KIND.to_string(),
                label: "Memory: reference project path".to_string(),
                detail: "content: Reference project source lives at /Users/example/reference-project."
                    .to_string(),
                selection_reason: "retrieved as a candidate for the current turn query".to_string(),
                rank: 1,
            },
            RetrievedMemoryCandidate {
                kind: RETRIEVED_WORKSPACE_MEMORY_KIND.to_string(),
                label: "Memory: reference project docs".to_string(),
                detail: "content: Reference project docs live under /Users/example/reference-project/docs."
                    .to_string(),
                selection_reason: "retrieved as a candidate for the current turn query".to_string(),
                rank: 2,
            },
        ];
        let rendered = render_retrieved_memory_context(
            retrieved
                .iter()
                .map(|candidate| RetrievedMemoryRenderItem {
                    label: candidate.label.as_str(),
                    detail: candidate.detail.as_str(),
                })
                .collect::<Vec<_>>()
                .as_slice(),
        )
        .expect("retrieved memory context should render");
        let exact_budget = estimate_text_tokens(&rendered);

        let result = memory_selection_for_test(
            &[],
            None,
            &[],
            &[],
            &[],
            &history,
            "session-1",
            "memory://vdb",
            &retrieved,
            &[],
            Some(exact_budget),
        );

        let selected = result
            .selected_items
            .iter()
            .filter(|item| item.kind == RETRIEVED_WORKSPACE_MEMORY_KIND)
            .collect::<Vec<_>>();
        assert_eq!(
            selected.len(),
            2,
            "shared memory-context overhead should be charged once, not once per candidate"
        );
        assert_eq!(
            selected
                .iter()
                .map(|item| item.budget_impact_tokens.unwrap_or_default())
                .sum::<usize>(),
            exact_budget
        );
    }

    #[test]
    fn direct_retrieved_memory_candidates_are_dropped_when_budget_is_tight() {
        let history = vec![Message {
            role: "user".to_string(),
            content: json!([{"type":"text","text":"Where is the reference project?"}]),
        }];
        let retrieved = vec![RetrievedMemoryCandidate {
            kind: RETRIEVED_THREAD_CONTEXT_KIND.to_string(),
            label: "Session Context session-1#3".to_string(),
            detail:
                "content: a long prior session observation that will exceed the one-token budget"
                    .to_string(),
            selection_reason: "retrieved as a candidate for the current turn query".to_string(),
            rank: 1,
        }];

        let result = memory_selection_for_test(
            &[],
            None,
            &[],
            &[],
            &[],
            &history,
            "session-1",
            "memory://vdb",
            &retrieved,
            &[],
            Some(1),
        );

        let dropped = result
            .dropped_items
            .iter()
            .find(|item| item.kind == RETRIEVED_THREAD_CONTEXT_KIND)
            .expect("direct retrieved thread context should be dropped by budget");
        assert!(
            dropped
                .dropped_reason
                .as_ref()
                .is_some_and(|reason| reason.reason().contains("memory-selection budget"))
        );
    }

    // ── Category completeness ──────────────────────────────────────────────

    #[test]
    fn memory_selection_reports_all_three_categories() {
        let history = vec![Message {
            role: "user".to_string(),
            content: json!("hello"),
        }];
        let result = memory_selection_for_test(
            &[],
            None,
            &[],
            &[],
            &[],
            &history,
            "session-1",
            "memory://vdb",
            &[],
            &[],
            Some(10_000),
        );

        // Selected: at least latest_user_request + thread_history (if budget allows)
        assert!(
            !result.selected_items.is_empty(),
            "should have selected items"
        );
        // Available: vector_memory should be there
        let available_kinds: Vec<&str> = result
            .available_items
            .iter()
            .map(|item| item.kind.as_str())
            .collect();
        assert!(
            available_kinds.contains(&"vector_memory"),
            "vector_memory should be in available"
        );
        // workspace_memory_available_item is also pushed when not already selected
        let has_workspace_available = available_kinds.contains(&"workspace_memory");
        assert!(
            has_workspace_available,
            "workspace_memory should be in available when no workspace prompt source is active"
        );
    }
}
