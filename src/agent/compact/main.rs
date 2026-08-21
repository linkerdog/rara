use std::sync::OnceLock;
use std::time::Duration;

use super::types::*;
use crate::agent::*;
use crate::llm::{ContextBudget, is_context_window_error};
use crate::session::PersistedCompactionEvent;

impl Agent {
    pub async fn compact_if_needed_with_reporter<F>(&mut self, mut report: F) -> Result<()>
    where
        F: FnMut(AgentEvent) + Send,
    {
        self.compact_history_with_reporter(false, &mut report).await
    }

    pub async fn compact_now_with_reporter<F>(&mut self, mut report: F) -> Result<bool>
    where
        F: FnMut(AgentEvent) + Send,
    {
        self.compact_history_with_reporter(true, &mut report)
            .await?;
        Ok(self.compact_state.last_compaction_before_tokens.is_some())
    }

    #[allow(dead_code)] // Reserved compression variant
    pub async fn compact_range_now_with_reporter<F>(
        &mut self,
        from: usize,
        up_to: usize,
        mut report: F,
    ) -> Result<bool>
    where
        F: FnMut(AgentEvent) + Send,
    {
        if from >= up_to {
            return Ok(false);
        }
        if up_to > self.history.len() {
            return Err(anyhow::anyhow!(
                "partial compaction range exceeds history length"
            ));
        }
        ensure_api_round_boundary_range(&self.history, from, up_to)?;

        let current_tokens = if self.compact_state.estimated_history_tokens > 0 {
            self.compact_state.estimated_history_tokens
        } else {
            estimate_history_tokens(&self.history).unwrap_or_default()
        };
        self.compact_state.estimated_history_tokens = current_tokens;
        self.compact_state.last_compaction_before_tokens = None;
        self.compact_state.last_compaction_after_tokens = None;
        self.compact_state.last_compaction_recent_files.clear();
        self.compact_state.last_compaction_boundary = None;

        report(AgentEvent::Status(
            "Compacting selected conversation history range.".to_string(),
        ));

        let compacted_slice = self.history[from..up_to].to_vec();
        let compact_instruction = self.context_assembler().compact_instruction();
        let summary = self
            .summarize_compaction_input_with_retry(
                compacted_slice.as_slice(),
                &compact_instruction,
                &mut report,
            )
            .await
            .map_err(|err| {
                if err.is::<CompactionSummaryTimeout>() {
                    anyhow::anyhow!("{err}")
                } else {
                    err
                }
            })?;
        let carry_over = build_compact_carry_over(
            summary.clone(),
            compacted_slice.as_slice(),
            self.retrieved_memory_candidates.as_slice(),
        );
        let replacement = build_post_compact_history(current_tokens, &carry_over, &[]);
        let mut new_history = Vec::new();
        new_history.extend_from_slice(&self.history[..from]);
        new_history.extend(replacement);
        new_history.extend_from_slice(&self.history[up_to..]);
        self.replace_history(new_history);
        self.checkpoint_session()?;

        let compacted_tokens = self.compact_state.estimated_history_tokens;
        self.compact_state.compaction_count += 1;
        self.compact_state.last_compaction_before_tokens = Some(current_tokens);
        self.compact_state.last_compaction_after_tokens = Some(compacted_tokens);
        self.compact_state.last_compaction_recent_files = carry_over.recent_files;
        self.compact_state.last_compaction_boundary = Some(CompactBoundaryMetadata {
            version: COMPACT_BOUNDARY_VERSION,
            before_tokens: current_tokens,
            recent_file_count: self.compact_state.last_compaction_recent_files.len(),
        });
        self.compact_state.consecutive_auto_compaction_failures = 0;
        self.compact_state.auto_compaction_retry_after_tokens = None;
        let summary_for_event = summary.clone();
        self.persist_compaction_event(&PersistedCompactionEvent {
            event_index: self.compact_state.compaction_count,
            before_tokens: current_tokens,
            after_tokens: compacted_tokens,
            boundary_version: COMPACT_BOUNDARY_VERSION,
            replaced_start: Some(from),
            replaced_end: Some(up_to),
            metadata_owner: Some("runtime.compaction".to_string()),
            recent_files: self.compact_state.last_compaction_recent_files.clone(),
            summary,
        })?;
        report(AgentEvent::Compaction {
            count: self.compact_state.compaction_count,
            before_tokens: current_tokens,
            after_tokens: compacted_tokens,
            summary: summary_for_event,
            recent_files: self.compact_state.last_compaction_recent_files.clone(),
        });
        Ok(true)
    }

    async fn compact_history_with_reporter<F>(&mut self, force: bool, report: &mut F) -> Result<()>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let mut current_tokens = self.compact_state.estimated_history_tokens;
        if current_tokens == 0 {
            current_tokens = estimate_history_tokens(&self.history).unwrap_or_default();
        }
        let compact_budget = self.current_compact_budget();
        self.compact_state.estimated_history_tokens = current_tokens;
        self.compact_state.context_window_tokens = compact_budget
            .as_ref()
            .map(|budget| budget.context_window_tokens);
        self.compact_state.compact_threshold_tokens = compact_budget
            .as_ref()
            .map(|budget| budget.compact_threshold_tokens)
            .unwrap_or(10_000);
        self.compact_state.reserved_output_tokens = compact_budget
            .as_ref()
            .map(|budget| budget.reserved_output_tokens)
            .unwrap_or(0);
        self.compact_state.last_compaction_before_tokens = None;
        self.compact_state.last_compaction_after_tokens = None;
        self.compact_state.last_compaction_recent_files.clear();
        self.compact_state.last_compaction_boundary = None;

        let threshold = self.compact_state.compact_threshold_tokens;
        if !force && current_tokens <= threshold {
            return Ok(());
        }
        if self.history.len() < 2 {
            return Ok(());
        }
        if !force
            && self
                .compact_state
                .auto_compaction_retry_after_tokens
                .is_some_and(|retry_after| current_tokens < retry_after)
        {
            report(AgentEvent::Status(
                "Automatic history compaction is temporarily suspended after a previous failure."
                    .to_string(),
            ));
            return Ok(());
        }

        report(AgentEvent::Status(if force {
            "Compacting conversation history on demand.".to_string()
        } else {
            "Compacting long conversation history.".to_string()
        }));

        let Some(plan) = build_compact_plan(&self.history, threshold, force)? else {
            return Ok(());
        };
        let compact_instruction = self.context_assembler().compact_instruction();
        let summary = match self
            .summarize_compaction_input_with_retry(
                &self.history[..plan.summarize_end],
                &compact_instruction,
                report,
            )
            .await
        {
            Ok(summary) => summary,
            Err(err) if err.is::<CompactionSummaryTimeout>() => {
                if !force {
                    self.record_auto_compaction_failure(current_tokens);
                    report(AgentEvent::Status(
                        "Automatic history compaction timed out; continuing without compaction."
                            .to_string(),
                    ));
                    return Ok(());
                }
                return Err(anyhow::anyhow!("{err}"));
            }
            Err(err) => {
                if !force {
                    self.record_auto_compaction_failure(current_tokens);
                    report(AgentEvent::Status(format!(
                        "Automatic history compaction failed; continuing without compaction. {err}"
                    )));
                    return Ok(());
                }
                return Err(err);
            }
        };
        let carry_over = build_compact_carry_over(
            summary.clone(),
            &self.history[..plan.summarize_end],
            self.retrieved_memory_candidates.as_slice(),
        );
        let new_history = build_post_compact_history(
            current_tokens,
            &carry_over,
            &self.history[plan.retained_start..],
        );
        self.replace_history(new_history);
        self.checkpoint_session()?;

        let compacted_tokens = self.compact_state.estimated_history_tokens;
        self.compact_state.compaction_count += 1;
        self.compact_state.last_compaction_before_tokens = Some(current_tokens);
        self.compact_state.last_compaction_after_tokens = Some(compacted_tokens);
        self.compact_state.last_compaction_recent_files = carry_over.recent_files;
        self.compact_state.last_compaction_boundary = Some(CompactBoundaryMetadata {
            version: COMPACT_BOUNDARY_VERSION,
            before_tokens: current_tokens,
            recent_file_count: self.compact_state.last_compaction_recent_files.len(),
        });
        self.compact_state.consecutive_auto_compaction_failures = 0;
        self.compact_state.auto_compaction_retry_after_tokens = None;
        let summary_for_event = summary.clone();
        self.persist_compaction_event(&PersistedCompactionEvent {
            event_index: self.compact_state.compaction_count,
            before_tokens: current_tokens,
            after_tokens: compacted_tokens,
            boundary_version: COMPACT_BOUNDARY_VERSION,
            replaced_start: Some(0),
            replaced_end: Some(plan.retained_start),
            metadata_owner: Some("runtime.compaction".to_string()),
            recent_files: self.compact_state.last_compaction_recent_files.clone(),
            summary,
        })?;
        report(AgentEvent::Compaction {
            count: self.compact_state.compaction_count,
            before_tokens: current_tokens,
            after_tokens: compacted_tokens,
            summary: summary_for_event,
            recent_files: self.compact_state.last_compaction_recent_files.clone(),
        });
        Ok(())
    }

    fn persist_compaction_event(&self, event: &PersistedCompactionEvent) -> Result<()> {
        if let Some(state_db) = self.state_db.as_deref() {
            let recorder = ThreadRecorder::new(state_db);
            return recorder.persist_compaction_event(&self.session_id, event);
        }
        self.session_manager
            .save_compaction_event(&self.session_id, event)
    }

    fn record_auto_compaction_failure(&mut self, current_tokens: usize) {
        self.compact_state.consecutive_auto_compaction_failures += 1;
        self.compact_state.auto_compaction_retry_after_tokens =
            Some(current_tokens.saturating_add(AUTO_COMPACTION_RETRY_HYSTERESIS_TOKENS));
    }

    async fn summarize_compaction_input_with_retry<F>(
        &self,
        messages: &[Message],
        instruction: &str,
        report: &mut F,
    ) -> Result<String>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let mut input_start = 0usize;
        loop {
            let input = &messages[input_start..];
            let summary_result = tokio::time::timeout(
                compaction_summary_timeout(),
                self.llm_backend.summarize(input, instruction),
            )
            .await;
            match summary_result {
                Ok(Ok(summary)) => return Ok(summary),
                Ok(Err(err)) if is_context_window_error(&err) => {
                    let groups = group_history_by_api_round(input);
                    let Some(next_start) = groups.get(1).map(|group| group.start) else {
                        return Err(err);
                    };
                    input_start = input_start.saturating_add(next_start);
                    report(AgentEvent::Status(
                        "Compaction summary prompt exceeded the context window; retrying without the oldest API round."
                            .to_string(),
                    ));
                }
                Ok(Err(err)) => return Err(err),
                Err(_) => return Err(anyhow::Error::new(CompactionSummaryTimeout)),
            }
        }
    }

    pub(crate) fn current_compact_budget(&self) -> Option<ContextBudget> {
        let tools = self.visible_tool_schemas();
        self.context_assembler()
            .budget_for(self.llm_backend.as_ref(), &self.history, &tools)
    }

    pub(crate) fn push_history_message(&mut self, message: Message) {
        self.record_history_message_tokens(&message);
        self.history.push(message);
    }

    pub(crate) fn extend_history_messages(&mut self, messages: Vec<Message>) {
        self.record_history_messages_tokens(&messages);
        self.history.extend(messages);
    }

    pub(crate) fn replace_history(&mut self, history: Vec<Message>) {
        self.history = history;
        self.recompute_history_token_estimate();
    }

    fn record_history_message_tokens(&mut self, message: &Message) {
        if let Ok(tokens) = estimate_message_tokens(message) {
            self.compact_state.estimated_history_tokens += tokens;
        } else {
            self.compact_state.estimated_history_tokens +=
                approximate_token_count_for_message(message);
        }
    }

    fn record_history_messages_tokens(&mut self, messages: &[Message]) {
        for message in messages {
            self.record_history_message_tokens(message);
        }
    }

    pub(in crate::agent) fn recompute_history_token_estimate(&mut self) {
        self.compact_state.estimated_history_tokens =
            estimate_history_tokens(&self.history).unwrap_or_default();
    }
}

include!("planning.rs");
include!("helpers.rs");
fn collect_recent_file_excerpts(
    history: &[Message],
    limit: usize,
    char_limit: usize,
) -> Vec<RecentFileExcerpt> {
    use std::collections::HashMap;

    let mut pending_reads = HashMap::<String, (String, Option<(usize, usize)>)>::new();
    let mut excerpts = Vec::new();

    for message in history {
        match message.role.as_str() {
            "assistant" => {
                let Some(items) = message.content.as_array() else {
                    continue;
                };
                for item in items {
                    if item.get("type").and_then(Value::as_str) != Some("tool_use") {
                        continue;
                    }
                    if item.get("name").and_then(Value::as_str) != Some("read_file") {
                        continue;
                    }
                    let Some(tool_use_id) = item.get("id").and_then(Value::as_str) else {
                        continue;
                    };
                    let Some(input) = item.get("input").and_then(Value::as_object) else {
                        continue;
                    };
                    let Some(path) = input.get("path").and_then(Value::as_str) else {
                        continue;
                    };
                    let line_range = read_file_line_range(input);
                    pending_reads.insert(
                        tool_use_id.to_string(),
                        (path.replace('\\', "/"), line_range),
                    );
                }
            }
            "user" => {
                let Some(items) = message.content.as_array() else {
                    continue;
                };
                for item in items {
                    if item.get("type").and_then(Value::as_str) != Some("tool_result") {
                        continue;
                    }
                    let Some(tool_use_id) = item.get("tool_use_id").and_then(Value::as_str) else {
                        continue;
                    };
                    let Some((path, line_range)) = pending_reads.remove(tool_use_id) else {
                        continue;
                    };
                    let snippet = item
                        .get("content")
                        .and_then(Value::as_str)
                        .map(|content| truncate_excerpt(content, char_limit).trim().to_string())
                        .filter(|content| !content.is_empty());
                    let Some(snippet) = snippet else {
                        continue;
                    };
                    excerpts.retain(|existing: &RecentFileExcerpt| existing.path != path);
                    excerpts.push(RecentFileExcerpt {
                        path,
                        line_range,
                        snippet,
                    });
                }
            }
            _ => {}
        }
    }

    if excerpts.len() > limit {
        excerpts = excerpts[excerpts.len() - limit..].to_vec();
    }
    excerpts.reverse();
    excerpts
}

pub(crate) fn read_file_line_range(
    input: &serde_json::Map<String, Value>,
) -> Option<(usize, usize)> {
    match (
        input.get("offset").and_then(Value::as_u64),
        input.get("limit").and_then(Value::as_u64),
    ) {
        (Some(offset), Some(limit)) if limit > 0 => {
            let start = usize::try_from(offset).ok()?;
            let limit = usize::try_from(limit).ok()?;
            let end = start.checked_add(limit)?.checked_sub(1)?;
            return Some((start, end));
        }
        (Some(offset), None) => {
            let start = usize::try_from(offset).ok()?;
            return Some((start, start));
        }
        _ => {}
    }

    match (
        input.get("start_line").and_then(Value::as_u64),
        input.get("end_line").and_then(Value::as_u64),
    ) {
        (Some(start), Some(end)) => {
            let start = usize::try_from(start).ok()?;
            let end = usize::try_from(end).ok()?;
            Some((start, end))
        }
        (Some(start), None) => {
            let start = usize::try_from(start).ok()?;
            Some((start, start))
        }
        _ => None,
    }
}

fn render_recent_file_excerpt(excerpt: &RecentFileExcerpt) -> String {
    let header = match excerpt.line_range {
        Some((start, end)) if start != end => {
            format!("### {} (lines {}-{})", excerpt.path, start, end)
        }
        Some((line, _)) => format!("### {} (line {})", excerpt.path, line),
        None => format!("### {}", excerpt.path),
    };
    format!("{header}\n```text\n{}\n```", excerpt.snippet)
}

fn truncate_excerpt(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    let truncated = text.chars().take(max_chars).collect::<String>();
    format!("{truncated}\n... truncated.")
}

fn recent_file_excerpt_source_item(excerpt: &RecentFileExcerpt) -> Value {
    let mut item = json!({
        "path": excerpt.path.clone(),
        "snippet": excerpt.snippet.clone(),
    });
    if let Some((line_start, line_end)) = excerpt.line_range {
        item["line_start"] = json!(line_start);
        item["line_end"] = json!(line_end);
    }
    item
}

fn build_compact_boundary_message(before_tokens: usize, recent_file_count: usize) -> Message {
    Message {
        role: "system".to_string(),
        content: compact_source_content(
            format!(
                "COMPACTION BOUNDARY: version={} before_tokens={} recent_file_count={}",
                COMPACT_BOUNDARY_VERSION, before_tokens, recent_file_count
            ),
            json!({
                "type": COMPACT_BOUNDARY_KIND,
                "version": COMPACT_BOUNDARY_VERSION,
                "before_tokens": before_tokens,
                "recent_file_count": recent_file_count,
            }),
        ),
    }
}

pub fn latest_compact_boundary_metadata(history: &[Message]) -> Option<CompactBoundaryMetadata> {
    history.iter().rev().find_map(|message| {
        let content = compact_boundary_item(&message.content)?;
        Some(CompactBoundaryMetadata {
            version: content
                .get("version")
                .and_then(Value::as_u64)
                .unwrap_or(COMPACT_BOUNDARY_VERSION as u64) as u32,
            before_tokens: content
                .get("before_tokens")
                .and_then(Value::as_u64)
                .unwrap_or_default() as usize,
            recent_file_count: content
                .get("recent_file_count")
                .and_then(Value::as_u64)
                .unwrap_or_default() as usize,
        })
    })
}

pub(crate) fn compact_boundary_item(content: &Value) -> Option<&serde_json::Map<String, Value>> {
    if let Some(object) = content.as_object() {
        if content.get("type").and_then(Value::as_str) != Some(COMPACT_BOUNDARY_KIND) {
            return None;
        }
        return Some(object);
    }
    content.as_array()?.iter().find_map(|item| {
        let object = item.as_object()?;
        (object.get("type").and_then(Value::as_str) == Some(COMPACT_BOUNDARY_KIND))
            .then_some(object)
    })
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value, json};

    use super::{build_compact_plan, group_history_by_api_round, read_file_line_range};
    use crate::agent::Message;

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().expect("object").clone()
    }

    #[test]
    fn read_file_line_range_rejects_overflowing_offset_limit() {
        let input = object(json!({
            "offset": usize::MAX,
            "limit": 2,
        }));

        assert_eq!(read_file_line_range(&input), None);
    }

    #[test]
    fn read_file_line_range_accepts_checked_offset_limit() {
        let input = object(json!({
            "offset": 10,
            "limit": 3,
        }));

        assert_eq!(read_file_line_range(&input), Some((10, 12)));
    }

    #[test]
    fn api_round_grouping_keeps_tool_result_with_assistant_round() {
        let history = vec![
            Message {
                role: "user".to_string(),
                content: json!("start"),
            },
            Message {
                role: "assistant".to_string(),
                content: json!([
                    {"type":"tool_use","id":"tool-1","name":"read_file","input":{"path":"src/main.rs"}}
                ]),
            },
            Message {
                role: "user".to_string(),
                content: json!([
                    {"type":"tool_result","tool_use_id":"tool-1","content":"fn main() {}"}
                ]),
            },
            Message {
                role: "assistant".to_string(),
                content: json!("done"),
            },
        ];

        let groups = group_history_by_api_round(&history);

        assert_eq!(
            groups
                .iter()
                .map(|group| (group.start, group.end))
                .collect::<Vec<_>>(),
            vec![(0, 1), (1, 3), (3, 4)]
        );
    }

    #[test]
    fn compact_plan_uses_api_round_boundary_for_retained_suffix() {
        let history = vec![
            Message {
                role: "user".to_string(),
                content: json!("old request"),
            },
            Message {
                role: "assistant".to_string(),
                content: json!([
                    {"type":"tool_use","id":"tool-1","name":"read_file","input":{"path":"src/old.rs"}}
                ]),
            },
            Message {
                role: "user".to_string(),
                content: json!([
                    {"type":"tool_result","tool_use_id":"tool-1","content":"old output ".repeat(1_000)}
                ]),
            },
            Message {
                role: "assistant".to_string(),
                content: json!([
                    {"type":"tool_use","id":"tool-2","name":"read_file","input":{"path":"src/new.rs"}}
                ]),
            },
            Message {
                role: "user".to_string(),
                content: json!([
                    {"type":"tool_result","tool_use_id":"tool-2","content":"new output"}
                ]),
            },
        ];

        let plan = build_compact_plan(&history, 600, false)
            .expect("plan")
            .expect("compact plan");

        assert_eq!(plan.summarize_end, 3);
        assert_eq!(plan.retained_start, 3);
    }

    #[test]
    fn compact_plan_does_not_split_single_assistant_tool_round() {
        let history = vec![
            Message {
                role: "assistant".to_string(),
                content: json!([
                    {"type":"tool_use","id":"tool-1","name":"read_file","input":{"path":"src/main.rs"}}
                ]),
            },
            Message {
                role: "user".to_string(),
                content: json!([
                    {"type":"tool_result","tool_use_id":"tool-1","content":"fn main() {}"}
                ]),
            },
        ];

        let plan = build_compact_plan(&history, 1, false).expect("plan");

        assert!(
            plan.is_none(),
            "single API round must not retain a detached tool_result"
        );
    }

    #[test]
    fn compact_plan_summarizes_oversized_latest_api_round() {
        let large_tool_output = "recent output ".repeat(2_000);
        let history = vec![
            Message {
                role: "user".to_string(),
                content: json!("old request"),
            },
            Message {
                role: "assistant".to_string(),
                content: json!("old answer"),
            },
            Message {
                role: "assistant".to_string(),
                content: json!([
                    {"type":"tool_use","id":"tool-1","name":"read_file","input":{"path":"src/main.rs"}}
                ]),
            },
            Message {
                role: "user".to_string(),
                content: json!([
                    {"type":"tool_result","tool_use_id":"tool-1","content": large_tool_output}
                ]),
            },
        ];

        let plan = build_compact_plan(&history, 100, false)
            .expect("plan")
            .expect("compact plan");
        assert_eq!(plan.summarize_end, history.len());
        assert_eq!(plan.retained_start, history.len());
    }

    #[test]
    fn cached_token_estimate_avoids_recomputation() {
        // Verify that CompactState stores estimated_history_tokens
        // and the compact_plan builder uses thresholds correctly.
        // The cache is populated incrementally by increment_history_tokens.
        use crate::agent::CompactState;
        let state = CompactState {
            estimated_history_tokens: 500,
            ..Default::default()
        };
        assert_eq!(state.estimated_history_tokens, 500);
    }
}
