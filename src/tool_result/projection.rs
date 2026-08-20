use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::content::tool_result_content_mut;
use crate::agent::Message;

const MICROCOMPACT_TOOL_RESULT_BUDGET: usize = 48_000;
const MICROCOMPACT_KEEP_RECENT_TOOL_RESULTS: usize = 6;
const ACTIVE_TURN_SUMMARY_CHARS: usize = 1_600;
const PRIOR_TURN_REFERENCE_CHARS: usize = 520;
const MINIMAL_REFERENCE_CHARS: usize = 280;
pub const MICROCOMPACT_CLEARED_MESSAGE: &str = "[Old tool result content cleared]";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResultProjectionPolicy {
    pub enabled: bool,
    pub budget_chars: usize,
    pub keep_recent: usize,
    pub cache_edit_eligible: bool,
}

impl Default for ToolResultProjectionPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            budget_chars: MICROCOMPACT_TOOL_RESULT_BUDGET,
            keep_recent: MICROCOMPACT_KEEP_RECENT_TOOL_RESULTS,
            cache_edit_eligible: false,
        }
    }
}

impl ToolResultProjectionPolicy {
    pub fn for_provider_cache_edit(mut self, cache_edit_supported: bool) -> Self {
        self.cache_edit_eligible = cache_edit_supported;
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolResultProjectionReport {
    pub original_chars: usize,
    pub projected_chars: usize,
    pub cleared_results: usize,
    pub summarized_results: usize,
    pub reference_only_results: usize,
    pub active_turn_kept_results: usize,
    pub kept_results: usize,
    pub cache_edit_eligible: bool,
    pub cache_edit_applied: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolUseMetadata {
    name: String,
    input: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolResultProjectionCandidate {
    message_index: usize,
    block_index: usize,
    tool_use_id: String,
    tool_name: String,
    tool_input: Value,
    content: String,
    chars: usize,
    active_turn: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProjectionState {
    Original,
    SemanticSummary,
    ReferenceOnly,
}

pub fn project_tool_results_for_context(
    messages: &[Message],
    policy: &ToolResultProjectionPolicy,
) -> (Vec<Message>, ToolResultProjectionReport) {
    if !policy.enabled || policy.budget_chars == 0 {
        return (
            messages.to_vec(),
            ToolResultProjectionReport {
                cache_edit_eligible: policy.cache_edit_eligible,
                ..ToolResultProjectionReport::default()
            },
        );
    }

    let tool_uses = compactable_tool_uses(messages);
    if tool_uses.is_empty() {
        return (
            messages.to_vec(),
            ToolResultProjectionReport {
                cache_edit_eligible: policy.cache_edit_eligible,
                ..ToolResultProjectionReport::default()
            },
        );
    }

    let active_turn_start = latest_user_text_message_index(messages);
    let candidates = projection_candidates(messages, &tool_uses, active_turn_start);
    let original_chars = candidates.iter().map(|candidate| candidate.chars).sum();
    let legacy_cleared_results = candidates
        .iter()
        .filter(|candidate| candidate.content.contains(MICROCOMPACT_CLEARED_MESSAGE))
        .count();
    if original_chars <= policy.budget_chars {
        return (
            messages.to_vec(),
            projection_report(
                &candidates,
                &vec![ProjectionState::Original; candidates.len()],
                original_chars,
                original_chars,
                legacy_cleared_results,
                policy,
            ),
        );
    }

    let keep_recent = policy.keep_recent.max(1);
    let keep_indices = candidates
        .iter()
        .enumerate()
        .rev()
        .take(keep_recent)
        .map(|(index, _)| index)
        .collect::<HashSet<_>>();
    let mut projected = messages.to_vec();
    let mut projected_chars = original_chars;
    let mut states = vec![ProjectionState::Original; candidates.len()];

    for (index, candidate) in candidates.iter().enumerate() {
        if projected_chars <= policy.budget_chars {
            break;
        }
        if candidate.active_turn || keep_indices.contains(&index) {
            continue;
        }
        replace_candidate(
            &mut projected,
            candidate,
            prior_turn_reference(candidate),
            ProjectionState::ReferenceOnly,
            &mut states[index],
            &mut projected_chars,
        );
    }

    for (index, candidate) in candidates.iter().enumerate() {
        if projected_chars <= policy.budget_chars {
            break;
        }
        if !candidate.active_turn || keep_indices.contains(&index) {
            continue;
        }
        replace_candidate(
            &mut projected,
            candidate,
            active_turn_semantic_summary(candidate),
            ProjectionState::SemanticSummary,
            &mut states[index],
            &mut projected_chars,
        );
    }

    for (index, candidate) in candidates.iter().enumerate() {
        if projected_chars <= policy.budget_chars {
            break;
        }
        if keep_indices.contains(&index) {
            continue;
        }
        replace_candidate(
            &mut projected,
            candidate,
            minimal_reference(candidate),
            ProjectionState::ReferenceOnly,
            &mut states[index],
            &mut projected_chars,
        );
    }

    (
        projected,
        projection_report(
            &candidates,
            &states,
            original_chars,
            projected_chars,
            legacy_cleared_results,
            policy,
        ),
    )
}

fn projection_report(
    candidates: &[ToolResultProjectionCandidate],
    states: &[ProjectionState],
    original_chars: usize,
    projected_chars: usize,
    cleared_results: usize,
    policy: &ToolResultProjectionPolicy,
) -> ToolResultProjectionReport {
    ToolResultProjectionReport {
        original_chars,
        projected_chars,
        cleared_results,
        summarized_results: states
            .iter()
            .filter(|state| **state == ProjectionState::SemanticSummary)
            .count(),
        reference_only_results: states
            .iter()
            .filter(|state| **state == ProjectionState::ReferenceOnly)
            .count(),
        active_turn_kept_results: candidates
            .iter()
            .zip(states)
            .filter(|(candidate, state)| {
                candidate.active_turn && **state == ProjectionState::Original
            })
            .count(),
        kept_results: states
            .iter()
            .filter(|state| **state == ProjectionState::Original)
            .count(),
        cache_edit_eligible: policy.cache_edit_eligible,
        cache_edit_applied: false,
    }
}

fn replace_candidate(
    projected: &mut [Message],
    candidate: &ToolResultProjectionCandidate,
    replacement: String,
    replacement_state: ProjectionState,
    state: &mut ProjectionState,
    projected_chars: &mut usize,
) {
    let Some(content) =
        tool_result_content_mut(projected, candidate.message_index, candidate.block_index)
    else {
        return;
    };
    let current_chars = content.chars().count();
    let replacement_chars = replacement.chars().count();
    if replacement_chars >= current_chars {
        return;
    }
    *projected_chars = projected_chars
        .saturating_sub(current_chars)
        .saturating_add(replacement_chars);
    *content = replacement;
    *state = replacement_state;
}

fn compactable_tool_uses(messages: &[Message]) -> HashMap<String, ToolUseMetadata> {
    let mut tool_uses = HashMap::new();
    for message in messages {
        if message.role != "assistant" {
            continue;
        }
        let Some(blocks) = message.content.as_array() else {
            continue;
        };
        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            let Some(id) = block.get("id").and_then(Value::as_str) else {
                continue;
            };
            let Some(name) = block.get("name").and_then(Value::as_str) else {
                continue;
            };
            if is_microcompactable_tool(name) {
                tool_uses.insert(
                    id.to_string(),
                    ToolUseMetadata {
                        name: name.to_string(),
                        input: block.get("input").cloned().unwrap_or(Value::Null),
                    },
                );
            }
        }
    }
    tool_uses
}

fn projection_candidates(
    messages: &[Message],
    tool_uses: &HashMap<String, ToolUseMetadata>,
    active_turn_start: Option<usize>,
) -> Vec<ToolResultProjectionCandidate> {
    let mut candidates = Vec::new();
    for (message_index, message) in messages.iter().enumerate() {
        if message.role != "user" {
            continue;
        }
        let Some(blocks) = message.content.as_array() else {
            continue;
        };
        for (block_index, block) in blocks.iter().enumerate() {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let Some(tool_use_id) = block.get("tool_use_id").and_then(Value::as_str) else {
                continue;
            };
            let Some(metadata) = tool_uses.get(tool_use_id) else {
                continue;
            };
            let Some(content) = block.get("content").and_then(Value::as_str) else {
                continue;
            };
            candidates.push(ToolResultProjectionCandidate {
                message_index,
                block_index,
                tool_use_id: tool_use_id.to_string(),
                tool_name: metadata.name.clone(),
                tool_input: metadata.input.clone(),
                content: content.to_string(),
                chars: content.chars().count(),
                active_turn: active_turn_start.is_none_or(|start| message_index > start),
            });
        }
    }
    candidates
}

fn latest_user_text_message_index(messages: &[Message]) -> Option<usize> {
    messages
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, message)| {
            (message.role == "user" && message_contains_text_request(message)).then_some(index)
        })
}

fn message_contains_text_request(message: &Message) -> bool {
    if message
        .content
        .as_str()
        .is_some_and(|text| !text.trim().is_empty())
    {
        return true;
    }
    message.content.as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item.get("type").and_then(Value::as_str) == Some("text")
                && item
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.trim().is_empty())
        })
    })
}

fn is_microcompactable_tool(name: &str) -> bool {
    matches!(
        name,
        "bash"
            | "read_file"
            | "grep"
            | "glob"
            | "web_search"
            | "web_fetch"
            | "apply_patch"
            | "write_file"
            | "replace"
            | "replace_lines"
    )
}

fn active_turn_semantic_summary(candidate: &ToolResultProjectionCandidate) -> String {
    let mut lines = vec![
        "[Tool result summary: active turn]".to_string(),
        format!("tool={}", candidate.tool_name),
        format!("tool_use_id={}", candidate.tool_use_id),
    ];
    push_input_line(&mut lines, &candidate.tool_input);
    if let Some(full_result) = full_result_line(&candidate.content) {
        lines.push(full_result.to_string());
    }
    lines.push("evidence:".to_string());
    lines.push(head_tail(&candidate.content, 650, 650));
    truncate_chars(&lines.join("\n"), ACTIVE_TURN_SUMMARY_CHARS)
}

fn prior_turn_reference(candidate: &ToolResultProjectionCandidate) -> String {
    let mut lines = vec![
        "[Tool result reference: prior turn]".to_string(),
        format!("tool={}", candidate.tool_name),
        format!("tool_use_id={}", candidate.tool_use_id),
    ];
    push_input_line(&mut lines, &candidate.tool_input);
    if let Some(summary) = first_evidence_line(&candidate.content) {
        lines.push(format!("summary={}", truncate_chars(summary, 180)));
    }
    if let Some(full_result) = full_result_line(&candidate.content) {
        lines.push(full_result.to_string());
    }
    truncate_chars(&lines.join("\n"), PRIOR_TURN_REFERENCE_CHARS)
}

fn minimal_reference(candidate: &ToolResultProjectionCandidate) -> String {
    let mut lines = vec![
        "[Tool result reference]".to_string(),
        format!("tool={}", candidate.tool_name),
        format!("tool_use_id={}", candidate.tool_use_id),
    ];
    push_input_line(&mut lines, &candidate.tool_input);
    if let Some(full_result) = full_result_line(&candidate.content) {
        lines.push(full_result.to_string());
    }
    truncate_chars(&lines.join("\n"), MINIMAL_REFERENCE_CHARS)
}

fn push_input_line(lines: &mut Vec<String>, input: &Value) {
    if input.is_null() {
        return;
    }
    let rendered = serde_json::to_string(input).unwrap_or_else(|_| input.to_string());
    lines.push(format!("input={}", truncate_chars(&rendered, 360)));
}

fn first_evidence_line(content: &str) -> Option<&str> {
    content.lines().map(str::trim).find(|line| {
        !line.is_empty() && *line != MICROCOMPACT_CLEARED_MESSAGE && !line.starts_with("reason=")
    })
}

fn full_result_line(content: &str) -> Option<&str> {
    content
        .lines()
        .find(|line| line.starts_with("full result:") || line.starts_with("full_result_path="))
}

fn head_tail(content: &str, head_chars: usize, tail_chars: usize) -> String {
    let total_chars = content.chars().count();
    if total_chars <= head_chars.saturating_add(tail_chars) {
        return content.to_string();
    }
    let head = content.chars().take(head_chars).collect::<String>();
    let tail = content
        .chars()
        .rev()
        .take(tail_chars)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    let omitted = total_chars.saturating_sub(head_chars + tail_chars);
    format!("{head}\n... [{omitted} chars omitted] ...\n{tail}")
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}
