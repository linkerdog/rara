use crate::agent::Message;
use crate::context::{
    DropReason, MemorySelectionContextView, MemorySelectionItemContextEntry,
    RetrievalBudgetContextView, RetrievalCandidateContextEntry, RetrievalOrchestrationView,
    RetrievalProviderStatus, RetrievalSourceContextEntry,
};
use crate::prompt::PromptSource;
use crate::workspace::WorkspaceMemory;

#[allow(clippy::too_many_arguments)]
// Retrieval status needs all provider inputs to produce a single ordered view.
pub(crate) fn retrieval_source_entries(
    workspace: &WorkspaceMemory,
    prompt_sources: &[PromptSource],
    history: &[Message],
    session_id: &str,
    vdb_uri: &str,
    mcp_resource_candidates: &[crate::context::RetrievalCandidate],
    hook_output_candidates: &[crate::context::RetrievalCandidate],
    graph_context_candidates: &[crate::context::RetrievalCandidate],
) -> Vec<RetrievalSourceContextEntry> {
    let workspace_memory_active = prompt_sources
        .iter()
        .any(|source| source.kind_label() == "local_memory");
    let workspace_memory_path = workspace.rara_dir.join("memory.md");
    let workspace_memory_exists = workspace.has_memory_file_cached();
    let workspace_memory_status = if workspace_memory_active {
        "active"
    } else if workspace_memory_exists {
        "available"
    } else {
        "missing"
    };
    let thread_history_status = if history.is_empty() {
        "empty"
    } else {
        "available"
    };
    let vector_memory_status = if vdb_uri.is_empty() {
        "missing"
    } else {
        "available"
    };
    let mcp_resource_status = if mcp_resource_candidates.is_empty() {
        "missing"
    } else {
        "available"
    };
    let hook_output_status = if hook_output_candidates.is_empty() {
        "missing"
    } else {
        "available"
    };
    let graph_context_status = if graph_context_candidates.is_empty() {
        "missing"
    } else {
        "available"
    };

    vec![
        RetrievalSourceContextEntry {
            order: 1,
            kind: "workspace_memory".to_string(),
            label: "Workspace Memory".to_string(),
            status: workspace_memory_status.to_string(),
            detail: workspace_memory_path.display().to_string(),
            inclusion_reason: match workspace_memory_status {
                "active" => "included now because the local workspace memory file was discovered as an explicit prompt source".to_string(),
                "available" => "available for future recall or prompt injection, but not active in the current turn".to_string(),
                _ => "no workspace memory file is available for recall or prompt injection".to_string(),
            },
        },
        RetrievalSourceContextEntry {
            order: 2,
            kind: "thread_history".to_string(),
            label: "Thread History".to_string(),
            status: thread_history_status.to_string(),
            detail: format!("session={} messages={}", session_id, history.len()),
            inclusion_reason: if history.is_empty() {
                "no persisted thread history is available for session-local recall yet".to_string()
            } else {
                "available as the session-local history source for restore and future recall surfaces".to_string()
            },
        },
        RetrievalSourceContextEntry {
            order: 3,
            kind: "vector_memory".to_string(),
            label: "Vector Memory Store".to_string(),
            status: vector_memory_status.to_string(),
            detail: vdb_uri.to_string(),
            inclusion_reason: if vector_memory_status == "available" {
                "configured as the durable vector-backed memory store for later retrieval, even though the current recall path is still limited".to_string()
            } else {
                "no vector-backed memory store is configured for retrieval".to_string()
            },
        },
        RetrievalSourceContextEntry {
            order: 4,
            kind: "mcp_resource".to_string(),
            label: "MCP Resources".to_string(),
            status: mcp_resource_status.to_string(),
            detail: format!("references={}", mcp_resource_candidates.len()),
            inclusion_reason: if mcp_resource_status == "available" {
                "available as protocol or MCP-provided resource references; resource bodies are not injected until selected by the retrieval pipeline".to_string()
            } else {
                "no MCP resource references are available for context selection".to_string()
            },
        },
        RetrievalSourceContextEntry {
            order: 5,
            kind: "hook_output".to_string(),
            label: "Hook Output".to_string(),
            status: hook_output_status.to_string(),
            detail: format!("outputs={}", hook_output_candidates.len()),
            inclusion_reason: if hook_output_status == "available" {
                "available as volatile hook output and injected directly as system context before the next model turn".to_string()
            } else {
                "no hook output is available for context selection".to_string()
            },
        },
        RetrievalSourceContextEntry {
            order: 6,
            kind: "graph_context".to_string(),
            label: "Graph Context".to_string(),
            status: graph_context_status.to_string(),
            detail: format!("contexts={}", graph_context_candidates.len()),
            inclusion_reason: if graph_context_status == "available" {
                "available as graph-expanded context; injection remains disabled until graph confidence policy is explicit".to_string()
            } else {
                "no graph-expanded context is available for context selection".to_string()
            },
        },
    ]
}

pub(crate) fn retrieval_orchestration_view(
    request_id: &str,
    query: &str,
    providers: &[RetrievalSourceContextEntry],
    memory_selection: &MemorySelectionContextView,
) -> RetrievalOrchestrationView {
    let selected = candidate_entries_from_memory_selection(
        "selected",
        &memory_selection.selected_items,
        MemorySelectionReasonSource::SelectionReason,
    );
    let available = candidate_entries_from_memory_selection(
        "available",
        &memory_selection.available_items,
        MemorySelectionReasonSource::DropReason,
    );
    let dropped = candidate_entries_from_memory_selection(
        "dropped",
        &memory_selection.dropped_items,
        MemorySelectionReasonSource::DropReason,
    );
    let mut candidates = Vec::new();
    candidates.extend(selected.clone());
    candidates.extend(available.clone());
    candidates.extend(dropped.clone());
    for (idx, candidate) in candidates.iter_mut().enumerate() {
        candidate.order = idx + 1;
    }

    RetrievalOrchestrationView {
        request_id: request_id.to_string(),
        query: query.to_string(),
        providers: providers.iter().map(provider_status_from_source).collect(),
        budget: RetrievalBudgetContextView {
            selection_budget_tokens: memory_selection.selection_budget_tokens,
            selected_tokens: sum_candidate_tokens(&selected),
            available_tokens: sum_candidate_tokens(&available),
            dropped_tokens: sum_candidate_tokens(&dropped),
        },
        candidates,
        selected,
        available,
        dropped,
    }
}

#[derive(Clone, Copy)]
enum MemorySelectionReasonSource {
    SelectionReason,
    DropReason,
}

fn candidate_entries_from_memory_selection(
    status: &str,
    items: &[MemorySelectionItemContextEntry],
    reason_source: MemorySelectionReasonSource,
) -> Vec<RetrievalCandidateContextEntry> {
    items
        .iter()
        .enumerate()
        .map(|(idx, item)| RetrievalCandidateContextEntry {
            order: idx + 1,
            kind: item.kind.clone(),
            label: item.label.clone(),
            detail: item.detail.clone(),
            status: status.to_string(),
            source_kind: source_kind_for_memory_selection_item(item.kind.as_str()).to_string(),
            budget_impact_tokens: item.budget_impact_tokens,
            reason: memory_selection_reason(item, reason_source),
        })
        .collect()
}

fn provider_status_from_source(source: &RetrievalSourceContextEntry) -> RetrievalProviderStatus {
    RetrievalProviderStatus {
        order: source.order,
        kind: source.kind.clone(),
        label: source.label.clone(),
        status: source.status.clone(),
        detail: source.detail.clone(),
        inclusion_reason: source.inclusion_reason.clone(),
    }
}

fn memory_selection_reason(
    item: &MemorySelectionItemContextEntry,
    reason_source: MemorySelectionReasonSource,
) -> String {
    match reason_source {
        MemorySelectionReasonSource::SelectionReason => item.selection_reason.clone(),
        MemorySelectionReasonSource::DropReason => item
            .dropped_reason
            .as_ref()
            .map(drop_reason_text)
            .unwrap_or_else(|| item.selection_reason.clone()),
    }
}

fn drop_reason_text(reason: &DropReason) -> String {
    reason.reason().to_string()
}

fn source_kind_for_memory_selection_item(kind: &str) -> &'static str {
    match kind {
        "retrieved_thread_context" => "session_context",
        "retrieved_workspace_memory" => "memory_record",
        "thread_history" => "thread_history",
        "vector_memory" => "vector_memory",
        "mcp_resource" => "mcp_resource",
        "hook_output" => "hook_output",
        "graph_context" => "graph_context",
        "workspace_memory" => "workspace_memory",
        "tool_retrieval_result" => "tool_result",
        _ => "runtime_context",
    }
}

fn sum_candidate_tokens(candidates: &[RetrievalCandidateContextEntry]) -> usize {
    candidates
        .iter()
        .map(|candidate| candidate.budget_impact_tokens.unwrap_or_default())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orchestration_view_projects_provider_status_and_candidate_sets() {
        let providers = vec![RetrievalSourceContextEntry {
            order: 1,
            kind: "vector_memory".to_string(),
            label: "Vector Memory Store".to_string(),
            status: "available".to_string(),
            detail: "memory://vdb".to_string(),
            inclusion_reason: "configured as durable memory".to_string(),
        }];
        let memory_selection = MemorySelectionContextView {
            selection_budget_tokens: Some(100),
            selected_items: vec![MemorySelectionItemContextEntry {
                order: 1,
                kind: "retrieved_workspace_memory".to_string(),
                label: "Memory: path".to_string(),
                detail: "content: path fact".to_string(),
                selection_reason: "retrieved for the current turn".to_string(),
                budget_impact_tokens: Some(11),
                dropped_reason: None,
            }],
            available_items: vec![MemorySelectionItemContextEntry {
                order: 1,
                kind: "thread_history".to_string(),
                label: "Thread History".to_string(),
                detail: "session=s1 messages=2".to_string(),
                selection_reason: "available for recall".to_string(),
                budget_impact_tokens: Some(7),
                dropped_reason: Some(DropReason::NotSelected {
                    reason: "compacted history already covers it".to_string(),
                }),
            }],
            dropped_items: vec![MemorySelectionItemContextEntry {
                order: 1,
                kind: "retrieved_thread_context".to_string(),
                label: "Session Context s1#1".to_string(),
                detail: "content: old context".to_string(),
                selection_reason: "retrieved session context".to_string(),
                budget_impact_tokens: Some(13),
                dropped_reason: Some(DropReason::BudgetExceeded {
                    reason: "budget_exceeded".to_string(),
                }),
            }],
        };

        let view = retrieval_orchestration_view(
            "s1",
            "where is the reference project?",
            providers.as_slice(),
            &memory_selection,
        );

        assert_eq!(view.request_id, "s1");
        assert_eq!(view.query, "where is the reference project?");
        assert_eq!(view.providers[0].kind, "vector_memory");
        assert_eq!(view.selected[0].source_kind, "memory_record");
        assert_eq!(view.available[0].source_kind, "thread_history");
        assert_eq!(
            view.available[0].reason,
            "compacted history already covers it"
        );
        assert_eq!(view.dropped[0].source_kind, "session_context");
        assert_eq!(view.dropped[0].reason, "budget_exceeded");
        assert_eq!(view.budget.selected_tokens, 11);
        assert_eq!(view.budget.available_tokens, 7);
        assert_eq!(view.budget.dropped_tokens, 13);
        assert_eq!(view.candidates.len(), 3);
        assert_eq!(view.candidates[0].status, "selected");
        assert_eq!(view.candidates[1].status, "available");
        assert_eq!(view.candidates[2].status, "dropped");
    }

    #[test]
    fn retrieval_sources_include_mcp_resource_references() {
        let workspace = crate::workspace::WorkspaceMemory::new().expect("workspace memory");
        let resources = vec![crate::context::mcp_resource_candidate(
            crate::context::McpResourceReference {
                server_name: "docs".to_string(),
                uri: "mcp://docs/rara".to_string(),
                title: Some("RARA Docs".to_string()),
                mime_type: Some("text/markdown".to_string()),
                token_estimate: Some(17),
                scope: Some("project".to_string()),
                source_path: Some("/workspace/rara/.mcp.json".to_string()),
            },
            1,
        )];

        let entries = retrieval_source_entries(
            &workspace,
            &[],
            &[],
            "session-1",
            "memory://vdb",
            &resources,
            &[],
            &[],
        );

        let mcp = entries
            .iter()
            .find(|entry| entry.kind == "mcp_resource")
            .expect("mcp resource provider should be visible");
        assert_eq!(mcp.status, "available");
        assert_eq!(mcp.detail, "references=1");
        assert!(mcp.inclusion_reason.contains("resource references"));
    }

    #[test]
    fn retrieval_sources_include_hook_and_graph_context_slots() {
        let workspace = crate::workspace::WorkspaceMemory::new().expect("workspace memory");
        let hook_output = vec![test_candidate("hook_output")];
        let graph_context = vec![test_candidate("graph_context")];

        let entries = retrieval_source_entries(
            &workspace,
            &[],
            &[],
            "session-1",
            "memory://vdb",
            &[],
            &hook_output,
            &graph_context,
        );

        let hook = entries
            .iter()
            .find(|entry| entry.kind == "hook_output")
            .expect("hook output provider should be visible");
        let graph = entries
            .iter()
            .find(|entry| entry.kind == "graph_context")
            .expect("graph context provider should be visible");
        assert_eq!(hook.status, "available");
        assert_eq!(hook.detail, "outputs=1");
        assert_eq!(graph.status, "available");
        assert_eq!(graph.detail, "contexts=1");
    }

    fn test_candidate(kind: &str) -> crate::context::RetrievalCandidate {
        crate::context::RetrievalCandidate {
            id: format!("{kind}:1"),
            source: crate::context::RetrievalSourceRef {
                source_type: kind.to_string(),
                source_id: Some(format!("{kind}-source")),
                source_path: None,
                source_uri: None,
                session_id: None,
                thread_id: None,
                workspace_id: None,
            },
            kind: kind.to_string(),
            scope: "runtime".to_string(),
            label: kind.to_string(),
            detail: format!("{kind} detail"),
            summary: None,
            rank: 1,
            score: None,
            priority: 60,
            dedupe_key: Some(format!("{kind}:dedupe")),
            budget_impact_tokens: Some(10),
            selection_reason: format!("{kind} selected"),
            availability_reason: format!("{kind} available"),
            not_selected_reason: format!("{kind} not selected"),
            selectable: false,
        }
    }
}
