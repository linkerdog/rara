use serde_json::json;

use super::*;
use crate::context::retrieval_provider::retrieval_candidate_from_retrieved_memory;
use crate::context::{
    RETRIEVED_THREAD_CONTEXT_KIND, RETRIEVED_WORKSPACE_MEMORY_KIND, RetrievalRequest,
    RetrievalSourceRef, RetrievedMemoryCandidate, retrieval_candidates,
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
    memory_uri: &str,
    retrieved_memory_candidates: &[RetrievedMemoryCandidate],
    file_search_candidates: &[RetrievalCandidate],
    selection_budget_tokens: Option<usize>,
) -> MemorySelectionContextView {
    let query = latest_user_request(history).unwrap_or_default();
    let request = RetrievalRequest {
        query: query.as_str(),
        session_id,
        history,
        memory_uri,
    };
    let candidates = retrieval_candidates(
        &request,
        retrieved_memory_candidates,
        file_search_candidates,
        &[],
        &[],
        &[],
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
fn local_memory_is_available_but_not_selectable() {
    let history: Vec<Message> = vec![];
    let result = memory_selection_for_test(
        &[],
        None,
        &[],
        &[],
        &[],
        &history,
        "session-1",
        "memory://local",
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
        available_kinds.contains(&"local_memory"),
        "local_memory should appear in available when a memory_handle URI is configured"
    );
    let vector_entry = result
        .available_items
        .iter()
        .find(|item| item.kind == "local_memory")
        .expect("local_memory should be present");
    assert!(
        vector_entry
            .dropped_reason
            .as_ref()
            .is_some_and(|r| r.reason().contains("not implemented")),
        "local_memory should explain it is not implemented yet"
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
                    "name": "retrieve_session_context",
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
                    "content": "Tool retrieve_session_context completed.\nPayload:\n{\n  \"relevant_context\": [\"Use shared bootstrap.\"]\n}"
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
        all_kinds.contains(&&"retrieved_thread_context"),
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
        "memory://local",
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
        "memory://local",
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
fn file_search_candidates_flow_through_memory_selection() {
    let history = vec![Message {
        role: "user".to_string(),
        content: json!([{"type":"text","text":"Open the provider tests"}]),
    }];
    let file_search = vec![RetrievalCandidate {
        id: "file_search:1:src-context-provider-tests-rs".to_string(),
        source: RetrievalSourceRef {
            source_type: "file_search".to_string(),
            source_id: None,
            source_path: Some("src/context/provider_tests.rs".to_string()),
            source_uri: None,
            session_id: None,
            thread_id: None,
            workspace_id: None,
        },
        kind: "file_search".to_string(),
        scope: "workspace".to_string(),
        label: "src/context/provider_tests.rs".to_string(),
        detail: "file_search(name_match, score=0.920); paths_only; content_not_read".to_string(),
        summary: None,
        rank: 1,
        score: Some(0.92),
        priority: 81,
        dedupe_key: None,
        budget_impact_tokens: Some(8),
        selection_reason:
            "paths-only candidate from file search (score 0.920); file contents were not read"
                .to_string(),
        availability_reason:
            "available because fuzzy path search matched the current turn query".to_string(),
        not_selected_reason:
            "not selected after ranking this low-priority paths-only file-search candidate"
                .to_string(),
        selectable: true,
    }];

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
        file_search.as_slice(),
        Some(10_000),
    );

    let selected = result
        .selected_items
        .iter()
        .find(|item| item.kind == "file_search")
        .expect("file-search candidate should enter memory selection");
    assert_eq!(selected.label, "src/context/provider_tests.rs");
    assert_eq!(selected.budget_impact_tokens, Some(8));
    assert!(
        selected.detail.contains("paths_only")
            && selected.detail.contains("content_not_read"),
        "file search selection must stay paths-only until an excerpt loader exists"
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
            detail: "content: Reference project source lives at /Users/example/reference-project."
                .to_string(),
            selection_reason: "retrieved as a candidate for the current turn query".to_string(),
            rank: 1,
        },
        RetrievedMemoryCandidate {
            kind: RETRIEVED_WORKSPACE_MEMORY_KIND.to_string(),
            label: "Memory: duplicate reference project path".to_string(),
            detail: "content: Reference project source lives at /Users/example/reference-project."
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
        "memory://local",
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
            detail:
                "content: Reference project docs live under /Users/example/reference-project/docs."
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
        "memory://local",
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
        detail: "content: a long prior session observation that will exceed the one-token budget"
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
        "memory://local",
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
        "memory://local",
        &[],
        &[],
        Some(10_000),
    );

    // Selected: at least latest_user_request + thread_history (if budget allows)
    assert!(
        !result.selected_items.is_empty(),
        "should have selected items"
    );
    // Available: local_memory should be there
    let available_kinds: Vec<&str> = result
        .available_items
        .iter()
        .map(|item| item.kind.as_str())
        .collect();
    assert!(
        available_kinds.contains(&"local_memory"),
        "local_memory should be in available"
    );
    // workspace_memory_available_item is also pushed when not already selected
    let has_workspace_available = available_kinds.contains(&"workspace_memory");
    assert!(
        has_workspace_available,
        "workspace_memory should be in available when no workspace prompt source is active"
    );
}
