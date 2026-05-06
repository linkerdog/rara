use serde_json::Value;

use crate::agent::Message;
use crate::context::CompactionSourceContextEntry;

pub(crate) fn compaction_source_entries(history: &[Message]) -> Vec<CompactionSourceContextEntry> {
    let mut entries = Vec::new();
    let mut compact_boundary_seen = false;

    for message in history {
        for item in compaction_items(&message.content) {
            let Some(item_type) = item.get("type").and_then(Value::as_str) else {
                continue;
            };
            match item_type {
                "compacted_summary" => entries.push(CompactionSourceContextEntry {
                    order: 0,
                    kind: "compacted_summary".to_string(),
                    label: "Compacted Summary".to_string(),
                    source_descriptor: "history.compaction.summary".to_string(),
                    detail: summarize_text_block(item.get("text").and_then(Value::as_str)),
                    inclusion_reason: "carried forward because the conversation history was compacted into a summary block".to_string(),
                }),
                "recent_files" => entries.push(CompactionSourceContextEntry {
                    order: 0,
                    kind: "recent_files".to_string(),
                    label: "Recent Files".to_string(),
                    source_descriptor: "history.compaction.recent_files".to_string(),
                    detail: summarize_recent_files(item.get("files").and_then(Value::as_array)),
                    inclusion_reason: "carried forward so the next turn keeps a lightweight view of recently touched files".to_string(),
                }),
                "recent_file_excerpts" => entries.push(CompactionSourceContextEntry {
                    order: 0,
                    kind: "recent_file_excerpts".to_string(),
                    label: "Recent File Excerpts".to_string(),
                    source_descriptor: "history.compaction.recent_file_excerpts".to_string(),
                    detail: summarize_recent_file_excerpts(item.get("files").and_then(Value::as_array)),
                    inclusion_reason: "carried forward so the next turn retains short excerpts from recently referenced files".to_string(),
                }),
                "compact_boundary" if !compact_boundary_seen => {
                    compact_boundary_seen = true;
                    entries.push(CompactionSourceContextEntry {
                        order: 0,
                        kind: "compact_boundary".to_string(),
                        label: "Compaction Boundary".to_string(),
                        source_descriptor: "history.compaction.boundary".to_string(),
                        detail: summarize_compact_boundary(item),
                        inclusion_reason: "recorded to explain where the latest compaction boundary cut the thread history".to_string(),
                    });
                }
                "compaction_carry_over" => {
                    if let Some(entry) = generic_carry_over_entry(item) {
                        entries.push(entry);
                    }
                }
                _ => {}
            }
        }
    }

    for (idx, entry) in entries.iter_mut().enumerate() {
        entry.order = idx + 1;
    }
    entries
}

fn generic_carry_over_entry(item: &Value) -> Option<CompactionSourceContextEntry> {
    let kind = item
        .get("kind")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let source_descriptor = item
        .get("source_descriptor")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| value.starts_with("history.compaction."))?;
    let label = item
        .get("label")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(kind);
    let detail = item
        .get("detail")
        .or_else(|| item.get("text"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("no detail");
    let inclusion_reason = item
        .get("inclusion_reason")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("carried forward through the post-compaction source descriptor stage");
    Some(CompactionSourceContextEntry {
        order: 0,
        kind: kind.to_string(),
        label: label.to_string(),
        source_descriptor: source_descriptor.to_string(),
        detail: detail.to_string(),
        inclusion_reason: inclusion_reason.to_string(),
    })
}

fn compaction_items(content: &Value) -> Vec<&Value> {
    if let Some(items) = content.as_array() {
        return items.iter().collect();
    }
    content
        .as_object()
        .map(|_| vec![content])
        .unwrap_or_default()
}

fn summarize_text_block(text: Option<&str>) -> String {
    text.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            let condensed = value.split_whitespace().collect::<Vec<_>>().join(" ");
            if condensed.len() > 96 {
                format!("{}...", &condensed[..93])
            } else {
                condensed
            }
        })
        .unwrap_or_else(|| "no summary text".to_string())
}

fn summarize_recent_files(files: Option<&Vec<Value>>) -> String {
    let files = files
        .into_iter()
        .flat_map(|items| items.iter())
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    match files.len() {
        0 => "no files".to_string(),
        1 => files[0].to_string(),
        _ => format!("{} (+{} more)", files[0], files.len() - 1),
    }
}

fn summarize_recent_file_excerpts(files: Option<&Vec<Value>>) -> String {
    let count = files.into_iter().flat_map(|items| items.iter()).count();
    match count {
        0 => "no excerpts".to_string(),
        1 => "1 excerpt".to_string(),
        _ => format!("{count} excerpts"),
    }
}

fn summarize_compact_boundary(item: &Value) -> String {
    let version = item
        .get("version")
        .and_then(Value::as_u64)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string());
    let before_tokens = item
        .get("before_tokens")
        .and_then(Value::as_u64)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string());
    let recent_file_count = item
        .get("recent_file_count")
        .and_then(Value::as_u64)
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string());
    format!("version={version} before_tokens={before_tokens} recent_files={recent_file_count}")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_generic_compaction_carry_over_source_items() {
        let history = vec![Message {
            role: "system".to_string(),
            content: json!([
                {
                    "type": "text",
                    "text": "MEMORY CARRY-OVER FROM COMPACTED HISTORY:\nUse the stable transcript path."
                },
                {
                    "type": "compaction_carry_over",
                    "kind": "compacted_memory",
                    "label": "Memory Carry-over",
                    "source_descriptor": "history.compaction.memory",
                    "detail": "Use the stable transcript path.",
                    "inclusion_reason": "carried forward from memory during compaction"
                }
            ]),
        }];

        let entries = compaction_source_entries(&history);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, "compacted_memory");
        assert_eq!(entries[0].label, "Memory Carry-over");
        assert_eq!(entries[0].source_descriptor, "history.compaction.memory");
        assert_eq!(entries[0].detail, "Use the stable transcript path.");
        assert_eq!(
            entries[0].inclusion_reason,
            "carried forward from memory during compaction"
        );
    }

    #[test]
    fn ignores_generic_carry_over_without_compaction_descriptor() {
        let history = vec![Message {
            role: "system".to_string(),
            content: json!([{
                "type": "compaction_carry_over",
                "kind": "compacted_memory",
                "source_descriptor": "workspace.memory",
                "detail": "wrong descriptor namespace"
            }]),
        }];

        assert!(compaction_source_entries(&history).is_empty());
    }
}
