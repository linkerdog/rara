use crate::agent::Message;
use crate::context::assembler::estimate_text_tokens;
use crate::context::{
    RETRIEVED_THREAD_CONTEXT_KIND, RETRIEVED_WORKSPACE_MEMORY_KIND, RetrievalCandidate,
    RetrievalSourceRef, RetrievedMemoryCandidate, RetrievedMemoryRenderItem,
    render_retrieved_memory_context_item,
};

#[derive(Debug, Clone, Copy)]
pub(crate) struct RetrievalRequest<'a> {
    pub query: &'a str,
    pub session_id: &'a str,
    pub history: &'a [Message],
    pub vdb_uri: &'a str,
}

pub(crate) trait RetrievalSourceProvider {
    fn source_kind(&self) -> &'static str;
    fn candidates(&self, request: &RetrievalRequest<'_>) -> Vec<RetrievalCandidate>;
}

pub(crate) fn retrieval_candidates(
    request: &RetrievalRequest<'_>,
    retrieved_memory_candidates: &[RetrievedMemoryCandidate],
    file_search_candidates: &[RetrievalCandidate],
) -> Vec<RetrievalCandidate> {
    let _query = request.query;
    let mut candidates = Vec::new();
    let direct_memory = DirectRetrievedMemoryProvider {
        candidates: retrieved_memory_candidates,
    };
    let retrieval_tools = RetrievalToolResultProvider;
    let thread_history = ThreadHistoryProvider;
    let vector_memory = VectorMemoryProvider;
    let file_search = PrecomputedFileSearchProvider {
        candidates: file_search_candidates,
    };

    let _source_order = [
        direct_memory.source_kind(),
        retrieval_tools.source_kind(),
        thread_history.source_kind(),
        vector_memory.source_kind(),
        file_search.source_kind(),
    ];
    candidates.extend(direct_memory.candidates(request));
    candidates.extend(retrieval_tools.candidates(request));
    candidates.extend(thread_history.candidates(request));
    candidates.extend(vector_memory.candidates(request));
    candidates.extend(file_search.candidates(request));
    candidates
}

pub(crate) struct DirectRetrievedMemoryProvider<'a> {
    candidates: &'a [RetrievedMemoryCandidate],
}

impl RetrievalSourceProvider for DirectRetrievedMemoryProvider<'_> {
    fn source_kind(&self) -> &'static str {
        "retrieved_memory"
    }

    fn candidates(&self, _request: &RetrievalRequest<'_>) -> Vec<RetrievalCandidate> {
        self.candidates
            .iter()
            .map(retrieval_candidate_from_retrieved_memory)
            .collect()
    }
}

pub(crate) struct PrecomputedFileSearchProvider<'a> {
    candidates: &'a [RetrievalCandidate],
}

impl RetrievalSourceProvider for PrecomputedFileSearchProvider<'_> {
    fn source_kind(&self) -> &'static str {
        "file_search"
    }

    fn candidates(&self, _request: &RetrievalRequest<'_>) -> Vec<RetrievalCandidate> {
        self.candidates.to_vec()
    }
}

pub(crate) struct RetrievalToolResultProvider;

impl RetrievalSourceProvider for RetrievalToolResultProvider {
    fn source_kind(&self) -> &'static str {
        "retrieval_tool_result"
    }

    fn candidates(&self, request: &RetrievalRequest<'_>) -> Vec<RetrievalCandidate> {
        retrieval_tool_candidates(request.history)
    }
}

pub(crate) struct ThreadHistoryProvider;

impl RetrievalSourceProvider for ThreadHistoryProvider {
    fn source_kind(&self) -> &'static str {
        "thread_history"
    }

    fn candidates(&self, request: &RetrievalRequest<'_>) -> Vec<RetrievalCandidate> {
        vec![thread_history_candidate(
            request.history,
            request.session_id,
        )]
    }
}

pub(crate) struct VectorMemoryProvider;

impl RetrievalSourceProvider for VectorMemoryProvider {
    fn source_kind(&self) -> &'static str {
        "vector_memory"
    }

    fn candidates(&self, request: &RetrievalRequest<'_>) -> Vec<RetrievalCandidate> {
        vec![vector_memory_candidate(request.vdb_uri)]
    }
}

pub(crate) fn retrieval_candidate_from_retrieved_memory(
    candidate: &RetrievedMemoryCandidate,
) -> RetrievalCandidate {
    let source_type = match candidate.kind.as_str() {
        RETRIEVED_THREAD_CONTEXT_KIND => "session_context",
        RETRIEVED_WORKSPACE_MEMORY_KIND => "memory_record",
        _ => "retrieved_memory",
    };
    let scope = match candidate.kind.as_str() {
        RETRIEVED_THREAD_CONTEXT_KIND => "thread",
        RETRIEVED_WORKSPACE_MEMORY_KIND => "workspace",
        _ => "unknown",
    };
    let priority = match candidate.kind.as_str() {
        RETRIEVED_THREAD_CONTEXT_KIND => 10,
        RETRIEVED_WORKSPACE_MEMORY_KIND => 20,
        _ => 25,
    } + candidate.rank;
    let dedupe_key = format!(
        "{}:{}:{}",
        source_type,
        candidate.kind,
        stable_retrieval_text_id(candidate.detail.as_str())
    );
    RetrievalCandidate {
        id: format!(
            "{}:{}:{}",
            source_type,
            candidate.rank,
            stable_retrieval_text_id(candidate.label.as_str())
        ),
        source: RetrievalSourceRef {
            source_type: source_type.to_string(),
            source_id: None,
            source_path: None,
            source_uri: None,
            session_id: None,
            thread_id: None,
            workspace_id: None,
        },
        kind: candidate.kind.clone(),
        scope: scope.to_string(),
        label: candidate.label.clone(),
        detail: candidate.detail.clone(),
        summary: None,
        rank: candidate.rank,
        score: None,
        priority,
        dedupe_key: Some(dedupe_key),
        budget_impact_tokens: Some(direct_retrieval_candidate_budget_impact(candidate)),
        selection_reason: candidate.selection_reason.clone(),
        availability_reason:
            "available because a retrieval provider returned this candidate for the current turn"
                .to_string(),
        not_selected_reason:
            "not selected after ranking the retrieved memory candidate against the current memory-selection budget"
                .to_string(),
        selectable: true,
    }
}

pub(crate) fn stable_retrieval_text_id(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .take(8)
        .collect::<Vec<_>>()
        .join("-")
}

fn direct_retrieval_candidate_budget_impact(candidate: &RetrievedMemoryCandidate) -> usize {
    let item = RetrievedMemoryRenderItem {
        label: candidate.label.as_str(),
        detail: candidate.detail.as_str(),
    };
    estimate_text_tokens(&render_retrieved_memory_context_item(item))
}

fn retrieval_tool_candidates(history: &[Message]) -> Vec<RetrievalCandidate> {
    let mut pending = std::collections::HashMap::new();
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
    pending: &mut std::collections::HashMap<String, (String, Option<String>)>,
    message: &Message,
) {
    let Some(items) = message.content.as_array() else {
        return;
    };
    for item in items {
        let Some(item_type) = item.get("type").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if item_type != "tool_use" {
            continue;
        }
        let Some(name) = item.get("name").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if !matches!(name, "retrieve_experience" | "retrieve_session_context") {
            continue;
        }
        let Some(tool_use_id) = item.get("id").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let query = item
            .get("input")
            .and_then(serde_json::Value::as_object)
            .and_then(|input| input.get("query"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        pending.insert(tool_use_id.to_string(), (name.to_string(), query));
    }
}

fn collect_retrieval_tool_results(
    pending: &mut std::collections::HashMap<String, (String, Option<String>)>,
    items: &mut Vec<RetrievalCandidate>,
    message: &Message,
) {
    let Some(blocks) = message.content.as_array() else {
        return;
    };
    for block in blocks {
        let Some(item_type) = block.get("type").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if item_type != "tool_result" {
            continue;
        }
        let Some(tool_use_id) = block.get("tool_use_id").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Some((name, query)) = pending.remove(tool_use_id) else {
            continue;
        };
        let content = block
            .get("content")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        items.push(retrieval_tool_candidate(
            name.as_str(),
            query.as_deref(),
            content,
        ));
    }
}

fn retrieval_tool_candidate(
    tool_name: &str,
    query: Option<&str>,
    content: &str,
) -> RetrievalCandidate {
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
            RetrievalCandidate {
                id: format!("tool:retrieve_experience:{}", stable_retrieval_text_id(&detail)),
                source: RetrievalSourceRef {
                    source_type: "tool_result".to_string(),
                    source_id: None,
                    source_path: None,
                    source_uri: None,
                    session_id: None,
                    thread_id: None,
                    workspace_id: None,
                },
                kind: "retrieved_workspace_memory".to_string(),
                scope: "workspace".to_string(),
                label: "Retrieved Experience".to_string(),
                detail: detail.clone(),
                summary: None,
                rank: 0,
                score: None,
                priority: 10,
                dedupe_key: None,
                budget_impact_tokens: Some(estimate_text_tokens(detail.as_str())),
                selection_reason: "selected because the retrieval tool returned relevant durable memory candidates for the current task".to_string(),
                availability_reason: "available because a retrieval tool result was found in thread history".to_string(),
                not_selected_reason: "not selected after ranking the retrieved workspace-memory candidates against the current memory-selection budget".to_string(),
                selectable: true,
            }
        }
        "retrieve_session_context" => {
            let summary = extract_json_string_field(content, "summary")
                .unwrap_or_else(|| "no session-context summary".to_string());
            let query = query.unwrap_or("query unavailable");
            let detail = format!("query={query}; summary: {summary}");
            RetrievalCandidate {
                id: format!(
                    "tool:retrieve_session_context:{}",
                    stable_retrieval_text_id(&detail)
                ),
                source: RetrievalSourceRef {
                    source_type: "tool_result".to_string(),
                    source_id: None,
                    source_path: None,
                    source_uri: None,
                    session_id: None,
                    thread_id: None,
                    workspace_id: None,
                },
                kind: "retrieved_thread_context".to_string(),
                scope: "thread".to_string(),
                label: "Retrieved Session Context".to_string(),
                detail: detail.clone(),
                summary: None,
                rank: 0,
                score: None,
                priority: 20,
                dedupe_key: None,
                budget_impact_tokens: Some(estimate_text_tokens(detail.as_str())),
                selection_reason: "selected because the retrieval tool returned focused thread-context material for the current task".to_string(),
                availability_reason: "available because a retrieval tool result was found in thread history".to_string(),
                not_selected_reason: "not selected after ranking the retrieved thread-context candidate against the current memory-selection budget".to_string(),
                selectable: true,
            }
        }
        other => {
            let detail = query
                .map(|query| format!("query={query}"))
                .unwrap_or_else(|| "query unavailable".to_string());
            RetrievalCandidate {
                id: format!("tool:{other}:{}", stable_retrieval_text_id(&detail)),
                source: RetrievalSourceRef {
                    source_type: "tool_result".to_string(),
                    source_id: None,
                    source_path: None,
                    source_uri: None,
                    session_id: None,
                    thread_id: None,
                    workspace_id: None,
                },
                kind: other.to_string(),
                scope: "thread".to_string(),
                label: other.to_string(),
                detail,
                summary: None,
                rank: 0,
                score: None,
                priority: 50,
                dedupe_key: None,
                budget_impact_tokens: Some(estimate_text_tokens(content)),
                selection_reason: "selected because a retrieval tool result was returned in the current thread history".to_string(),
                availability_reason: "available because a retrieval tool result was found in thread history".to_string(),
                not_selected_reason: "not selected after ranking the retrieval candidate against the current memory-selection budget".to_string(),
                selectable: true,
            }
        }
    }
}

fn thread_history_candidate(history: &[Message], session_id: &str) -> RetrievalCandidate {
    let detail = format!("session={session_id} messages={}", history.len());
    RetrievalCandidate {
        id: format!("thread_history:{session_id}"),
        source: RetrievalSourceRef {
            source_type: "thread_history".to_string(),
            source_id: None,
            source_path: None,
            source_uri: None,
            session_id: Some(session_id.to_string()),
            thread_id: None,
            workspace_id: None,
        },
        kind: "thread_history".to_string(),
        scope: "thread".to_string(),
        label: "Thread History".to_string(),
        detail: detail.clone(),
        summary: None,
        rank: 0,
        score: None,
        priority: 30,
        dedupe_key: None,
        budget_impact_tokens: Some(estimate_text_tokens(detail.as_str())),
        selection_reason:
            "thread history remains available as a recall source even when only active-turn state is currently injected"
                .to_string(),
        availability_reason: if history.is_empty() {
            "no thread history is available for selection".to_string()
        } else {
            "raw thread history was not selected directly because the current turn already has sufficient active-turn and compacted-history context".to_string()
        },
        not_selected_reason: if history.is_empty() {
            "no thread history is available for selection".to_string()
        } else {
            "raw thread history was not selected directly because the current turn already has sufficient active-turn and compacted-history context".to_string()
        },
        selectable: !history.is_empty(),
    }
}

fn vector_memory_candidate(vdb_uri: &str) -> RetrievalCandidate {
    let configured = !vdb_uri.is_empty();
    RetrievalCandidate {
        id: "vector_memory".to_string(),
        source: RetrievalSourceRef {
            source_type: "vector_memory".to_string(),
            source_id: None,
            source_path: None,
            source_uri: configured.then(|| vdb_uri.to_string()),
            session_id: None,
            thread_id: None,
            workspace_id: None,
        },
        kind: "vector_memory".to_string(),
        scope: "workspace".to_string(),
        label: "Vector Memory Store".to_string(),
        detail: if configured {
            vdb_uri.to_string()
        } else {
            "-".to_string()
        },
        summary: None,
        rank: 0,
        score: None,
        priority: 40,
        dedupe_key: None,
        budget_impact_tokens: None,
        selection_reason:
            "the vector-backed memory slot is part of the selection contract even before full ranked retrieval is implemented"
                .to_string(),
        availability_reason: if configured {
            "not selected because vector-backed candidate ranking is not implemented yet".to_string()
        } else {
            "no vector-backed memory store is configured".to_string()
        },
        not_selected_reason: if configured {
            "not selected because vector-backed candidate ranking is not implemented yet".to_string()
        } else {
            "no vector-backed memory store is configured".to_string()
        },
        selectable: false,
    }
}

fn extract_json_array_strings(content: &str, key: &str) -> Vec<String> {
    extract_tool_result_payload(content)
        .and_then(|payload| {
            payload
                .get(key)
                .and_then(serde_json::Value::as_array)
                .cloned()
        })
        .into_iter()
        .flatten()
        .filter_map(|item| item.as_str().map(str::trim).map(str::to_string))
        .filter(|value| !value.is_empty())
        .collect()
}

fn extract_json_string_field(content: &str, key: &str) -> Option<String> {
    extract_tool_result_payload(content)
        .and_then(|payload| {
            payload
                .get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn extract_tool_result_payload(content: &str) -> Option<serde_json::Value> {
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
    fn retrieval_request_keeps_current_turn_source_inputs() {
        let history = vec![Message {
            role: "user".to_string(),
            content: json!("where is the path?"),
        }];
        let request = RetrievalRequest {
            query: "where is the path?",
            session_id: "session-1",
            history: &history,
            vdb_uri: "memory://vdb",
        };

        assert_eq!(request.query, "where is the path?");
        assert_eq!(request.session_id, "session-1");
        assert_eq!(request.history.len(), 1);
        assert_eq!(request.vdb_uri, "memory://vdb");
    }

    #[test]
    fn provider_boundary_collects_current_sources_in_stable_order() {
        let history = vec![Message {
            role: "user".to_string(),
            content: json!("where is the path?"),
        }];
        let request = RetrievalRequest {
            query: "where is the path?",
            session_id: "session-1",
            history: &history,
            vdb_uri: "memory://vdb",
        };
        let retrieved = vec![RetrievedMemoryCandidate {
            kind: RETRIEVED_WORKSPACE_MEMORY_KIND.to_string(),
            label: "Memory: path".to_string(),
            detail: "content: /tmp/project".to_string(),
            selection_reason: "retrieved".to_string(),
            rank: 1,
        }];
        let file = vec![RetrievalCandidate {
            id: "file:src/main.rs".to_string(),
            source: RetrievalSourceRef {
                source_type: "file_search".to_string(),
                source_id: None,
                source_path: Some("src/main.rs".to_string()),
                source_uri: None,
                session_id: None,
                thread_id: None,
                workspace_id: None,
            },
            kind: "file_search".to_string(),
            scope: "workspace".to_string(),
            label: "src/main.rs".to_string(),
            detail: "file_search(name_match, score=1.000)".to_string(),
            summary: None,
            rank: 1,
            score: Some(1.0),
            priority: 31,
            dedupe_key: Some("file_search:src/main.rs".to_string()),
            budget_impact_tokens: Some(9),
            selection_reason: "candidate from file search (score 1.000)".to_string(),
            availability_reason: "available because file search matched the current turn query"
                .to_string(),
            not_selected_reason:
                "not selected after ranking the file-search candidate against the current memory-selection budget"
                    .to_string(),
            selectable: true,
        }];

        let memory_provider = DirectRetrievedMemoryProvider {
            candidates: &retrieved,
        };
        let file_provider = PrecomputedFileSearchProvider { candidates: &file };
        assert_eq!(memory_provider.source_kind(), "retrieved_memory");
        assert_eq!(file_provider.source_kind(), "file_search");

        let candidates = retrieval_candidates(&request, &retrieved, &file);

        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.kind.as_str())
                .collect::<Vec<_>>(),
            vec![
                RETRIEVED_WORKSPACE_MEMORY_KIND,
                "thread_history",
                "vector_memory",
                "file_search"
            ]
        );
        assert_eq!(candidates[0].source.source_type, "memory_record");
        assert_eq!(candidates[3].source.source_type, "file_search");
    }

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
}
