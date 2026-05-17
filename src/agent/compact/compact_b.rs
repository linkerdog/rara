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

fn approximate_message_char_count(message: &Message) -> usize {
    if let Some(text) = message.content.as_str() {
        text.len()
    } else {
        message.content.to_string().len()
    }
}

fn approximate_token_count_for_message(message: &Message) -> usize {
    approximate_message_char_count(message)
        .saturating_div(4)
        .max(1)
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

