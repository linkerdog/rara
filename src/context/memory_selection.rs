use std::collections::HashMap;

use serde_json::Value;

use crate::agent::{Message, PlanStepStatus};
use crate::context::assembler::{
    RuntimeInteractionInput, estimate_text_tokens, latest_tool_results, latest_user_request,
};
use crate::context::{
    CompactionSourceContextEntry, DropReason, MemorySelectionContextView,
    MemorySelectionItemContextEntry, RETRIEVED_THREAD_CONTEXT_KIND,
    RETRIEVED_WORKSPACE_MEMORY_KIND, RetrievedMemoryCandidate, RetrievedMemoryRenderItem,
    is_retrieved_memory_kind, render_retrieved_memory_context,
    render_retrieved_memory_context_item,
};
use crate::prompt::PromptSource;

pub(crate) fn memory_selection(
    prompt_sources: &[PromptSource],
    plan_explanation: Option<&str>,
    plan_steps: &[(PlanStepStatus, String)],
    pending_interactions: &[RuntimeInteractionInput],
    compacted_history: &[CompactionSourceContextEntry],
    history: &[Message],
    session_id: &str,
    vdb_uri: &str,
    retrieved_memory_candidates: &[RetrievedMemoryCandidate],
    file_search_candidates: &[MemorySelectionItemContextEntry],
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
        merge_file_search_candidates(
            retrieval_memory_candidates(history, session_id, vdb_uri, retrieved_memory_candidates),
            file_search_candidates,
        ),
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

fn retrieval_memory_candidates(
    history: &[Message],
    session_id: &str,
    vdb_uri: &str,
    retrieved_memory_candidates: &[RetrievedMemoryCandidate],
) -> Vec<MemorySelectionCandidate> {
    let mut candidates = direct_retrieved_memory_candidates(retrieved_memory_candidates);
    candidates.extend(retrieval_tool_candidates(history));
    candidates.push(thread_history_candidate(history, session_id));
    candidates.push(vector_memory_candidate(vdb_uri));
    candidates
}

fn direct_retrieved_memory_candidates(
    retrieved_memory_candidates: &[RetrievedMemoryCandidate],
) -> Vec<MemorySelectionCandidate> {
    retrieved_memory_candidates
        .iter()
        .map(|candidate| {
            let base_priority = match candidate.kind.as_str() {
                RETRIEVED_THREAD_CONTEXT_KIND => 10,
                RETRIEVED_WORKSPACE_MEMORY_KIND => 20,
                _ => 25,
            };
            MemorySelectionCandidate {
                kind: candidate.kind.clone(),
                label: candidate.label.clone(),
                detail: candidate.detail.clone(),
                selection_reason: candidate.selection_reason.clone(),
                budget_impact_tokens: Some(direct_retrieved_memory_budget_impact(candidate)),
                priority: base_priority + candidate.rank,
                selectable: true,
                dropped_reason: DropReason::NotSelected {
                    reason:
                        "not selected after ranking the retrieved memory candidate against the current memory-selection budget"
                            .to_string(),
                },
            }
        })
        .collect()
}

fn direct_retrieved_memory_budget_impact(candidate: &RetrievedMemoryCandidate) -> usize {
    let item = RetrievedMemoryRenderItem {
        label: candidate.label.as_str(),
        detail: candidate.detail.as_str(),
    };
    estimate_text_tokens(&render_retrieved_memory_context_item(item))
}

fn retrieval_tool_candidates(history: &[Message]) -> Vec<MemorySelectionCandidate> {
    let mut pending = HashMap::new();
    let mut items = Vec::new();

    for message in history {
        match message.role.as_str() {
            "assistant" => collect_pending_retrieval_tool_uses(&mut pending, message),
            "user" => collect_retrieval_tool_results(&mut pending, &mut items, message),
            _ => {}
        }
    }

    items
}

fn collect_pending_retrieval_tool_uses(
    pending: &mut HashMap<String, (String, Option<String>)>,
    message: &Message,
) {
    let Some(items) = message.content.as_array() else {
        return;
    };
    for item in items {
        let Some(item_type) = item.get("type").and_then(Value::as_str) else {
            continue;
        };
        if item_type != "tool_use" {
            continue;
        }
        let Some(name) = item.get("name").and_then(Value::as_str) else {
            continue;
        };
        if !matches!(name, "retrieve_experience" | "retrieve_session_context") {
            continue;
        }
        let Some(tool_use_id) = item.get("id").and_then(Value::as_str) else {
            continue;
        };
        let query = item
            .get("input")
            .and_then(Value::as_object)
            .and_then(|input| input.get("query"))
            .and_then(Value::as_str)
            .map(str::to_string);
        pending.insert(tool_use_id.to_string(), (name.to_string(), query));
    }
}

fn collect_retrieval_tool_results(
    pending: &mut HashMap<String, (String, Option<String>)>,
    items: &mut Vec<MemorySelectionCandidate>,
    message: &Message,
) {
    let Some(blocks) = message.content.as_array() else {
        return;
    };
    for block in blocks {
        let Some(item_type) = block.get("type").and_then(Value::as_str) else {
            continue;
        };
        if item_type != "tool_result" {
            continue;
        }
        let Some(tool_use_id) = block.get("tool_use_id").and_then(Value::as_str) else {
            continue;
        };
        let Some((name, query)) = pending.remove(tool_use_id) else {
            continue;
        };
        let content = block
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default();
        items.push(retrieval_tool_candidate(
            name.as_str(),
            query.as_deref(),
            content,
        ));
    }
}

/// Merge file-search candidates into the memory-selection candidate pool.
/// Each entry is converted to a `MemorySelectionCandidate` with priority
/// derived from its sort order (lower order = higher priority).
fn merge_file_search_candidates(
    mut candidates: Vec<MemorySelectionCandidate>,
    file_search: &[MemorySelectionItemContextEntry],
) -> Vec<MemorySelectionCandidate> {
    for entry in file_search {
        candidates.push(MemorySelectionCandidate {
            kind: entry.kind.clone(),
            label: entry.label.clone(),
            detail: entry.detail.clone(),
            selection_reason: entry.selection_reason.clone(),
            budget_impact_tokens: entry.budget_impact_tokens,
            priority: 30 + entry.order,
            selectable: true,
            dropped_reason: DropReason::NotSelected {
                reason:
                    "not selected after ranking the file-search candidate against the current memory-selection budget"
                        .to_string(),
            },
        });
    }
    candidates
}

fn select_memory_candidates(
    mut candidates: Vec<MemorySelectionCandidate>,
    selection_budget_tokens: Option<usize>,
    fixed_selected_kinds: &[String],
) -> MemorySelectionDecision {
    candidates.sort_by_key(|candidate| candidate.priority);
    let mut remaining_budget = selection_budget_tokens;
    let has_compacted_history = fixed_selected_kinds
        .iter()
        .any(|kind| is_compacted_history_kind(kind.as_str()));
    let mut decision = MemorySelectionDecision::default();
    let mut selected_kinds = fixed_selected_kinds.to_vec();
    let mut selected_retrieved_items: Vec<(String, String)> = Vec::new();

    for candidate in candidates {
        let candidate_budget_impact =
            candidate_budget_impact_tokens(&candidate, selected_retrieved_items.as_slice());
        let is_retrieved_memory = is_retrieved_memory_kind(candidate.kind.as_str());
        let selected_retrieved_item =
            is_retrieved_memory.then(|| (candidate.label.clone(), candidate.detail.clone()));
        let should_drop: Option<DropReason> = if !candidate.selectable {
            Some(candidate.dropped_reason.clone())
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

fn thread_history_candidate(history: &[Message], session_id: &str) -> MemorySelectionCandidate {
    MemorySelectionCandidate {
        kind: "thread_history".to_string(),
        label: "Thread History".to_string(),
        detail: format!("session={session_id} messages={}", history.len()),
        selection_reason: "thread history remains available as a recall source even when only active-turn state is currently injected".to_string(),
        budget_impact_tokens: Some(estimate_text_tokens(
            format!("session={session_id} messages={}", history.len()).as_str(),
        )),
        priority: 30,
        selectable: !history.is_empty(),
        dropped_reason: DropReason::NotSelected { reason: if history.is_empty() {
            "no thread history is available for selection".to_string()
        } else {
            "raw thread history was not selected directly because the current turn already has sufficient active-turn and compacted-history context".to_string()
        }},
    }
}

fn vector_memory_candidate(vdb_uri: &str) -> MemorySelectionCandidate {
    MemorySelectionCandidate {
        kind: "vector_memory".to_string(),
        label: "Vector Memory Store".to_string(),
        detail: if vdb_uri.is_empty() {
            "-".to_string()
        } else {
            vdb_uri.to_string()
        },
        selection_reason: "the vector-backed memory slot is part of the selection contract even before full ranked retrieval is implemented".to_string(),
        budget_impact_tokens: None,
        priority: 40,
        selectable: false,
        dropped_reason: DropReason::NotSelected { reason: if vdb_uri.is_empty() {
            "no vector-backed memory store is configured".to_string()
        } else {
            "not selected because vector-backed candidate ranking is not implemented yet".to_string()
        }},
    }
}

fn retrieval_tool_candidate(
    tool_name: &str,
    query: Option<&str>,
    content: &str,
) -> MemorySelectionCandidate {
    match tool_name {
        "retrieve_experience" => {
            let experiences = extract_json_array_strings(content, "relevant_experiences");
            let preview = if experiences.is_empty() {
                "no recalled experiences".to_string()
            } else {
                format!(
                    "recalled={} item(s); preview: {}",
                    experiences.len(),
                    experiences.join(" | ")
                )
            };
            let query = query.unwrap_or("query unavailable");
            let detail = format!("query={query}; {preview}");
            MemorySelectionCandidate {
                kind: "retrieved_workspace_memory".to_string(),
                label: "Retrieved Experience".to_string(),
                budget_impact_tokens: Some(estimate_text_tokens(detail.as_str())),
                detail,
                selection_reason: "selected because the retrieval tool returned relevant durable memory candidates for the current task".to_string(),
                priority: 10,
                selectable: true,
                dropped_reason: DropReason::NotSelected { reason: "not selected after ranking the retrieved workspace-memory candidates against the current memory-selection budget".to_string() },
            }
        }
        "retrieve_session_context" => {
            let summary = extract_json_string_field(content, "summary")
                .unwrap_or_else(|| "no session-context summary".to_string());
            let query = query.unwrap_or("query unavailable");
            let detail = format!("query={query}; summary: {summary}");
            MemorySelectionCandidate {
                kind: "retrieved_thread_context".to_string(),
                label: "Retrieved Session Context".to_string(),
                budget_impact_tokens: Some(estimate_text_tokens(detail.as_str())),
                detail,
                selection_reason: "selected because the retrieval tool returned focused thread-context material for the current task".to_string(),
                priority: 20,
                selectable: true,
                dropped_reason: DropReason::NotSelected { reason: "not selected after ranking the retrieved thread-context candidate against the current memory-selection budget".to_string() },
            }
        }
        other => MemorySelectionCandidate {
            kind: other.to_string(),
            label: other.to_string(),
            detail: query
                .map(|query| format!("query={query}"))
                .unwrap_or_else(|| "query unavailable".to_string()),
            selection_reason: "selected because a retrieval tool result was returned in the current thread history".to_string(),
            budget_impact_tokens: Some(estimate_text_tokens(content)),
            priority: 50,
            selectable: true,
            dropped_reason: DropReason::NotSelected { reason: "not selected after ranking the retrieval candidate against the current memory-selection budget".to_string() },
        },
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

fn extract_json_array_strings(content: &str, key: &str) -> Vec<String> {
    extract_tool_result_payload(content)
        .and_then(|payload| payload.get(key).and_then(Value::as_array).cloned())
        .into_iter()
        .flatten()
        .filter_map(|item| item.as_str().map(str::trim).map(str::to_string))
        .filter(|value| !value.is_empty())
        .collect()
}

fn extract_json_string_field(content: &str, key: &str) -> Option<String> {
    extract_tool_result_payload(content)
        .and_then(|payload| payload.get(key).and_then(Value::as_str).map(str::to_string))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn extract_tool_result_payload(content: &str) -> Option<Value> {
    let payload = content
        .split_once("Payload:\n")
        .map(|(_, payload)| payload)
        .unwrap_or(content)
        .trim();
    serde_json::from_str(payload).ok()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn extract_tool_result_payload_falls_back_to_plain_json_content() {
        let payload = extract_tool_result_payload(
            r#"{
                "status": "ok",
                "summary": "plain json payload without wrapper"
            }"#,
        )
        .expect("payload should parse from raw json");

        assert_eq!(
            payload.get("status").and_then(serde_json::Value::as_str),
            Some("ok")
        );
        assert_eq!(
            payload.get("summary").and_then(serde_json::Value::as_str),
            Some("plain json payload without wrapper")
        );
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
        let result = memory_selection(
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
        let result = memory_selection(
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
        let result = memory_selection(
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
        let result = memory_selection(
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
        let result = memory_selection(
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
        let result = memory_selection(
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

        let result = memory_selection(
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

        let result = memory_selection(
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

        let result = memory_selection(
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
        let result = memory_selection(
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
