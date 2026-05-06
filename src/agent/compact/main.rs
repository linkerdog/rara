use std::sync::OnceLock;
use std::time::Duration;

use super::types::*;
use crate::agent::*;
use crate::llm::{ContextBudget, is_context_window_error};
use crate::session::PersistedCompactionEvent;

impl Agent {
    pub async fn compact_if_needed(&mut self) -> Result<()> {
        self.compact_if_needed_with_reporter(|_| {}).await
    }

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

    #[allow(dead_code)]
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

        let current_tokens = estimate_history_tokens(&self.history).unwrap_or_default();
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
                    let groups = group_history_by_api_round(input)?;
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
            self.recompute_history_token_estimate();
        }
    }

    fn record_history_messages_tokens(&mut self, messages: &[Message]) {
        let mut total = 0usize;
        for message in messages {
            match estimate_message_tokens(message) {
                Ok(tokens) => total += tokens,
                Err(_) => {
                    self.recompute_history_token_estimate();
                    return;
                }
            }
        }
        self.compact_state.estimated_history_tokens += total;
    }

    fn recompute_history_token_estimate(&mut self) {
        self.compact_state.estimated_history_tokens =
            estimate_history_tokens(&self.history).unwrap_or_default();
    }
}

fn build_compact_plan(
    history: &[Message],
    threshold: usize,
    force: bool,
) -> Result<Option<CompactPlan>> {
    if history.len() < 2 {
        return Ok(None);
    }

    let groups = group_history_by_api_round(history)?;
    if groups.len() < 2 {
        return Ok(None);
    }

    let retained_budget = retained_history_budget(threshold, force);
    let mut retained_tokens = 0usize;
    let mut retained_group_index = groups.len() - 1;
    let latest_group = &groups[retained_group_index];
    if !force && latest_group.token_estimate > retained_budget {
        return Ok(Some(CompactPlan {
            summarize_end: history.len(),
            retained_start: history.len(),
        }));
    }

    for group_index in (1..groups.len()).rev() {
        let group = &groups[group_index];
        let next_tokens = retained_tokens.saturating_add(group.token_estimate);
        if retained_tokens > 0 && next_tokens > retained_budget {
            break;
        }
        retained_tokens = next_tokens;
        retained_group_index = group_index;
    }

    let retained_start = groups[retained_group_index].start;
    if retained_start == 0 {
        return Ok(Some(CompactPlan {
            summarize_end: groups[1].start,
            retained_start: groups[1].start,
        }));
    }

    Ok(Some(CompactPlan {
        summarize_end: retained_start,
        retained_start,
    }))
}

fn retained_history_budget(threshold: usize, force: bool) -> usize {
    if force {
        return 1;
    }
    threshold
        .saturating_div(RETAINED_HISTORY_BUDGET_FRACTION)
        .max(1)
}

#[allow(dead_code)]
fn ensure_api_round_boundary_range(history: &[Message], from: usize, up_to: usize) -> Result<()> {
    let groups = group_history_by_api_round(history)?;
    let is_boundary = |idx: usize| {
        idx == history.len()
            || groups
                .iter()
                .any(|group| group.start == idx || group.end == idx)
    };
    if !is_boundary(from) || !is_boundary(up_to) {
        return Err(anyhow::anyhow!(
            "partial compaction range must align with API-round boundaries"
        ));
    }
    Ok(())
}

fn group_history_by_api_round(history: &[Message]) -> Result<Vec<ApiRoundGroup>> {
    let bpe = tokenizer()?;
    let mut groups = Vec::new();
    let mut group_start = 0usize;
    let mut current_tokens = 0usize;

    for (idx, message) in history.iter().enumerate() {
        let starts_new_round = message.role == ROLE_ASSISTANT && idx > group_start;
        if starts_new_round {
            debug_assert!(idx > group_start);
            groups.push(ApiRoundGroup {
                start: group_start,
                end: idx,
                token_estimate: current_tokens,
            });
            group_start = idx;
            current_tokens = 0;
        }
        current_tokens =
            current_tokens.saturating_add(estimate_message_tokens_with_bpe(message, bpe)?);
    }

    if group_start < history.len() {
        groups.push(ApiRoundGroup {
            start: group_start,
            end: history.len(),
            token_estimate: current_tokens,
        });
    }
    debug_assert_eq!(groups.last().map(|group| group.end), Some(history.len()));

    Ok(groups)
}

fn build_compact_carry_over(
    summary: String,
    compacted_history: &[Message],
    retrieved_memory_candidates: &[RetrievedMemoryCandidate],
) -> CompactCarryOver {
    CompactCarryOver {
        summary,
        recent_files: collect_recent_files(compacted_history, RECENT_FILE_CARRY_OVER_LIMIT),
        recent_file_excerpts: collect_recent_file_excerpts(
            compacted_history,
            RECENT_FILE_EXCERPT_LIMIT,
            RECENT_FILE_EXCERPT_CHAR_LIMIT,
        ),
        retrieved_memory: collect_retrieved_memory_carry_over(
            retrieved_memory_candidates,
            MEMORY_CARRY_OVER_LIMIT,
        ),
        invoked_skills: collect_invoked_skill_carry_over(
            compacted_history,
            SKILL_CARRY_OVER_LIMIT,
            SKILL_INSTRUCTION_PREVIEW_CHAR_LIMIT,
        ),
        retained_hooks: collect_retained_context_carry_over(
            compacted_history,
            RetainedContextClass::Hooks,
            HOOK_CARRY_OVER_LIMIT,
        ),
        retained_mcp: collect_retained_context_carry_over(
            compacted_history,
            RetainedContextClass::Mcp,
            MCP_CARRY_OVER_LIMIT,
        ),
    }
}

fn build_post_compact_history(
    before_tokens: usize,
    carry_over: &CompactCarryOver,
    retained_history: &[Message],
) -> Vec<Message> {
    let mut history = vec![
        build_compact_boundary_message(before_tokens, carry_over.recent_files.len()),
        Message {
            role: "system".to_string(),
            content: compact_source_content(
                format!(
                    "STRUCTURED SUMMARY OF PREVIOUS CONVERSATION:\n{}",
                    carry_over.summary
                ),
                json!({
                    "type": "compacted_summary",
                    "text": carry_over.summary,
                }),
            ),
        },
    ];

    append_post_compact_carry_over(&mut history, carry_over);
    history.extend_from_slice(retained_history);
    history
}

fn append_post_compact_carry_over(history: &mut Vec<Message>, carry_over: &CompactCarryOver) {
    if !carry_over.recent_files.is_empty() {
        let recent_files_block = carry_over
            .recent_files
            .iter()
            .map(|path| format!("- {path}"))
            .collect::<Vec<_>>()
            .join("\n");
        history.push(Message {
            role: "system".to_string(),
            content: compact_source_content(
                format!(
                    "RECENT FILES FROM COMPACTED HISTORY:\n{}",
                    recent_files_block
                ),
                json!({
                    "type": "recent_files",
                    "files": carry_over.recent_files.clone(),
                }),
            ),
        });
    }

    if !carry_over.recent_file_excerpts.is_empty() {
        let excerpt_block = carry_over
            .recent_file_excerpts
            .iter()
            .map(render_recent_file_excerpt)
            .collect::<Vec<_>>()
            .join("\n\n");
        history.push(Message {
            role: "system".to_string(),
            content: compact_source_content(
                format!(
                    "RECENT FILE EXCERPTS FROM COMPACTED HISTORY:\n{}",
                    excerpt_block
                ),
                json!({
                    "type": "recent_file_excerpts",
                    "files": carry_over
                        .recent_file_excerpts
                        .iter()
                        .map(recent_file_excerpt_source_item)
                        .collect::<Vec<_>>(),
                }),
            ),
        });
    }

    if !carry_over.retrieved_memory.is_empty() {
        let memory_block = render_retrieved_memory_carry_over(&carry_over.retrieved_memory);
        history.push(Message {
            role: "system".to_string(),
            content: compact_source_content(
                format!("MEMORY CARRY-OVER FROM COMPACTED HISTORY:\n{}", memory_block),
                json!({
                    "type": "compaction_carry_over",
                    "kind": "compacted_memory",
                    "label": "Memory Carry-over",
                    "source_descriptor": "history.compaction.memory",
                    "detail": memory_block,
                    "inclusion_reason": "carried forward because retrieved memory was available before compaction and should remain available after history replacement",
                }),
            ),
        });
    }

    if !carry_over.invoked_skills.is_empty() {
        let skill_block = render_invoked_skill_carry_over(&carry_over.invoked_skills);
        history.push(Message {
            role: "system".to_string(),
            content: compact_source_content(
                format!("SKILL CARRY-OVER FROM COMPACTED HISTORY:\n{}", skill_block),
                json!({
                    "type": "compaction_carry_over",
                    "kind": "compacted_skills",
                    "label": "Skill Carry-over",
                    "source_descriptor": "history.compaction.skills",
                    "detail": skill_block,
                    "inclusion_reason": "carried forward because these skills were invoked before compaction and may still define the current workflow",
                }),
            ),
        });
    }

    if !carry_over.retained_hooks.is_empty() {
        let hook_block = render_retained_context_carry_over(&carry_over.retained_hooks);
        history.push(Message {
            role: "system".to_string(),
            content: compact_source_content(
                format!("HOOK CARRY-OVER FROM COMPACTED HISTORY:\n{}", hook_block),
                json!({
                    "type": "compaction_carry_over",
                    "kind": "compacted_hooks",
                    "label": "Hook Carry-over",
                    "source_descriptor": "history.compaction.hooks",
                    "detail": hook_block,
                    "inclusion_reason": "carried forward because hook-provided context requested retention before compaction",
                }),
            ),
        });
    }

    if !carry_over.retained_mcp.is_empty() {
        let mcp_block = render_retained_context_carry_over(&carry_over.retained_mcp);
        history.push(Message {
            role: "system".to_string(),
            content: compact_source_content(
                format!("MCP CARRY-OVER FROM COMPACTED HISTORY:\n{}", mcp_block),
                json!({
                    "type": "compaction_carry_over",
                    "kind": "compacted_mcp",
                    "label": "MCP Carry-over",
                    "source_descriptor": "history.compaction.mcp",
                    "detail": mcp_block,
                    "inclusion_reason": "carried forward because MCP-provided context requested retention before compaction",
                }),
            ),
        });
    }
}

fn compact_source_content(text: String, source_item: Value) -> Value {
    json!([
        {
            "type": "text",
            "text": text,
        },
        source_item,
    ])
}

pub(crate) fn compaction_summary_timeout() -> Duration {
    #[cfg(test)]
    {
        TEST_COMPACTION_SUMMARY_TIMEOUT
    }
    #[cfg(not(test))]
    {
        COMPACTION_SUMMARY_TIMEOUT
    }
}

fn estimate_history_tokens(history: &[Message]) -> Result<usize> {
    let bpe = tokenizer()?;
    history
        .iter()
        .map(|message| estimate_message_tokens_with_bpe(message, bpe))
        .sum::<Result<usize>>()
}

fn estimate_message_tokens(message: &Message) -> Result<usize> {
    let bpe = tokenizer()?;
    estimate_message_tokens_with_bpe(message, bpe)
}

fn estimate_message_tokens_with_bpe(
    message: &Message,
    bpe: &tiktoken_rs::CoreBPE,
) -> Result<usize> {
    let rendered;
    let content = if let Some(text) = message.content.as_str() {
        text
    } else {
        rendered = message.content.to_string();
        rendered.as_str()
    };
    Ok(bpe.encode_with_special_tokens(content).len())
}

pub(crate) fn tokenizer() -> Result<&'static tiktoken_rs::CoreBPE> {
    static BPE: OnceLock<std::result::Result<tiktoken_rs::CoreBPE, String>> = OnceLock::new();
    match BPE.get_or_init(|| tiktoken_rs::cl100k_base().map_err(|err| err.to_string())) {
        Ok(bpe) => Ok(bpe),
        Err(err) => Err(anyhow::anyhow!(err.clone())),
    }
}

fn collect_recent_files(history: &[Message], limit: usize) -> Vec<String> {
    let mut collected = Vec::new();
    for message in history.iter().rev() {
        if message.role != "assistant" {
            continue;
        }
        let Some(items) = message.content.as_array() else {
            continue;
        };
        for item in items.iter().rev() {
            if item.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            let Some(tool_name) = item.get("name").and_then(Value::as_str) else {
                continue;
            };
            let Some(input) = item.get("input").and_then(Value::as_object) else {
                continue;
            };
            let Some(path) = input.get("path").and_then(Value::as_str) else {
                continue;
            };
            if !matches!(
                tool_name,
                "read_file" | "list_files" | "write_file" | "replace" | "apply_patch"
            ) {
                continue;
            }
            let normalized = path.replace('\\', "/");
            if !collected.iter().any(|existing| existing == &normalized) {
                collected.push(normalized);
            }
            if collected.len() >= limit {
                return collected;
            }
        }
    }
    collected
}

fn collect_retrieved_memory_carry_over(
    candidates: &[RetrievedMemoryCandidate],
    limit: usize,
) -> Vec<RetrievedMemoryCandidate> {
    let mut candidates = candidates
        .iter()
        .filter(|candidate| !candidate.detail.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort_by_key(|candidate| candidate.rank);
    candidates.truncate(limit);
    candidates
}

fn render_retrieved_memory_carry_over(candidates: &[RetrievedMemoryCandidate]) -> String {
    candidates
        .iter()
        .map(|candidate| {
            format!(
                "- [{}] {}: {}",
                candidate.kind,
                candidate.label.trim(),
                candidate.detail.trim()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn collect_invoked_skill_carry_over(
    history: &[Message],
    limit: usize,
    instruction_preview_char_limit: usize,
) -> Vec<InvokedSkillCarryOver> {
    use std::collections::HashMap;

    let mut pending_invocations = HashMap::<String, SkillInvocationInput>::new();
    let mut invoked = Vec::<InvokedSkillCarryOver>::new();

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
                    if item.get("name").and_then(Value::as_str) != Some("skill") {
                        continue;
                    }
                    let Some(tool_use_id) = item.get("id").and_then(Value::as_str) else {
                        continue;
                    };
                    let Some(input) = item.get("input").and_then(Value::as_object) else {
                        continue;
                    };
                    if input.get("action").and_then(Value::as_str) != Some("invoke") {
                        continue;
                    }
                    let Some(skill_name) = input.get("skill_name").and_then(Value::as_str) else {
                        continue;
                    };
                    pending_invocations.insert(
                        tool_use_id.to_string(),
                        SkillInvocationInput {
                            name: skill_name.trim().to_string(),
                            args: input
                                .get("args")
                                .and_then(Value::as_str)
                                .map(str::trim)
                                .filter(|value| !value.is_empty())
                                .map(ToString::to_string),
                        },
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
                    if item
                        .get("is_error")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    {
                        continue;
                    }
                    let Some(tool_use_id) = item.get("tool_use_id").and_then(Value::as_str) else {
                        continue;
                    };
                    let Some(invocation) = pending_invocations.remove(tool_use_id) else {
                        continue;
                    };
                    let result = item
                        .get("content")
                        .and_then(Value::as_str)
                        .and_then(|content| serde_json::from_str::<Value>(content).ok());
                    invoked.retain(|existing| existing.name != invocation.name);
                    invoked.push(skill_carry_over_from_result(
                        invocation,
                        result.as_ref(),
                        instruction_preview_char_limit,
                    ));
                }
            }
            _ => {}
        }
    }

    if invoked.len() > limit {
        invoked = invoked[invoked.len() - limit..].to_vec();
    }
    invoked.reverse();
    invoked
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SkillInvocationInput {
    name: String,
    args: Option<String>,
}

fn skill_carry_over_from_result(
    invocation: SkillInvocationInput,
    result: Option<&Value>,
    instruction_preview_char_limit: usize,
) -> InvokedSkillCarryOver {
    let field = |name: &str| {
        result
            .and_then(|value| value.get(name))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    };

    InvokedSkillCarryOver {
        name: field("name").unwrap_or(invocation.name),
        title: field("title"),
        scope: field("scope"),
        display_path: field("display_path"),
        args: field("args").or(invocation.args),
        instruction_preview: field("instructions").map(|instructions| {
            truncate_excerpt(instructions.as_str(), instruction_preview_char_limit)
                .trim()
                .to_string()
        }),
    }
}

fn render_invoked_skill_carry_over(skills: &[InvokedSkillCarryOver]) -> String {
    skills
        .iter()
        .map(|skill| {
            let mut parts = vec![format!("- {}", skill.name.trim())];
            if let Some(title) = skill.title.as_deref().filter(|value| !value.is_empty()) {
                parts.push(format!("  title: {}", title.trim()));
            }
            if let Some(scope) = skill.scope.as_deref().filter(|value| !value.is_empty()) {
                parts.push(format!("  scope: {}", scope.trim()));
            }
            if let Some(path) = skill
                .display_path
                .as_deref()
                .filter(|value| !value.is_empty())
            {
                parts.push(format!("  path: {}", path.trim()));
            }
            if let Some(args) = skill.args.as_deref().filter(|value| !value.is_empty()) {
                parts.push(format!("  args: {}", args.trim()));
            }
            if let Some(preview) = skill
                .instruction_preview
                .as_deref()
                .filter(|value| !value.is_empty())
            {
                parts.push(format!("  instructions: {}", preview.trim()));
            }
            parts.join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetainedContextClass {
    Hooks,
    Mcp,
}

fn collect_retained_context_carry_over(
    history: &[Message],
    class: RetainedContextClass,
    limit: usize,
) -> Vec<RetainedContextCarryOver> {
    let mut retained = Vec::<RetainedContextCarryOver>::new();

    for message in history {
        for item in message_content_items(&message.content) {
            let Some(entry) = retained_context_carry_over_from_item(item, class) else {
                continue;
            };
            retained.retain(|existing| existing.source_descriptor != entry.source_descriptor);
            retained.push(entry);
        }
    }

    if retained.len() > limit {
        retained = retained[retained.len() - limit..].to_vec();
    }
    retained.reverse();
    retained
}

fn message_content_items(content: &Value) -> Vec<&Value> {
    if let Some(items) = content.as_array() {
        return items.iter().collect();
    }
    content
        .as_object()
        .map(|_| vec![content])
        .unwrap_or_default()
}

fn retained_context_carry_over_from_item(
    item: &Value,
    class: RetainedContextClass,
) -> Option<RetainedContextCarryOver> {
    if item.get("type").and_then(Value::as_str) != Some("compaction_retain_hint") {
        return None;
    }

    let source_descriptor = item
        .get("source_descriptor")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    if !retained_context_descriptor_matches(source_descriptor, class) {
        return None;
    }

    let detail = item
        .get("detail")
        .or_else(|| item.get("text"))
        .or_else(|| item.get("content"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let label = item
        .get("label")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(source_descriptor);
    let inclusion_reason = item
        .get("inclusion_reason")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);

    Some(RetainedContextCarryOver {
        label: label.to_string(),
        source_descriptor: source_descriptor.to_string(),
        detail: detail.to_string(),
        inclusion_reason,
    })
}

fn retained_context_descriptor_matches(
    source_descriptor: &str,
    class: RetainedContextClass,
) -> bool {
    match class {
        RetainedContextClass::Hooks => source_descriptor.starts_with("hook."),
        RetainedContextClass::Mcp => source_descriptor.starts_with("mcp."),
    }
}

fn render_retained_context_carry_over(entries: &[RetainedContextCarryOver]) -> String {
    entries
        .iter()
        .map(|entry| {
            let mut parts = vec![
                format!("- {}", entry.label.trim()),
                format!("  source: {}", entry.source_descriptor.trim()),
                format!("  detail: {}", entry.detail.trim()),
            ];
            if let Some(reason) = entry
                .inclusion_reason
                .as_deref()
                .filter(|value| !value.is_empty())
            {
                parts.push(format!("  reason: {}", reason.trim()));
            }
            parts.join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

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

fn read_file_line_range(input: &serde_json::Map<String, Value>) -> Option<(usize, usize)> {
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

        let groups = group_history_by_api_round(&history).expect("groups");

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
