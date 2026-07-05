pub(crate) fn build_compact_plan(
    history: &[Message],
    threshold: usize,
    force: bool,
) -> Result<Option<CompactPlan>> {
    if history.len() < 2 {
        return Ok(None);
    }

    let groups = group_history_by_api_round(history);
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

#[allow(dead_code)] // Reserved compact strategy
fn ensure_api_round_boundary_range(history: &[Message], from: usize, up_to: usize) -> Result<()> {
    let groups = group_history_by_api_round(history);
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

pub(crate) fn group_history_by_api_round(history: &[Message]) -> Vec<ApiRoundGroup> {
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
                token_estimate: current_tokens.max(1),
            });
            group_start = idx;
            current_tokens = 0;
        }
        current_tokens =
            current_tokens.saturating_add(approximate_token_count_for_message(message));
    }

    if group_start < history.len() {
        groups.push(ApiRoundGroup {
            start: group_start,
            end: history.len(),
            token_estimate: current_tokens.max(1),
        });
    }
    debug_assert_eq!(groups.last().map(|group| group.end), Some(history.len()));

    groups
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

