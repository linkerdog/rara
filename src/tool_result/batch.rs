use super::content::{tool_result_content_candidates, tool_result_content_mut};
use super::render::{head_tail_text, truncate_text};
use crate::agent::Message;

pub(super) const TOOL_RESULT_BATCH_BUDGET: usize = 24_000;
const BATCH_PREVIEW_HEAD: usize = 1_000;
const BATCH_PREVIEW_TAIL: usize = 1_000;

pub fn enforce_tool_result_batch_budget(mut messages: Vec<Message>) -> Vec<Message> {
    let mut candidates = tool_result_content_candidates(&messages);
    let mut total_chars: usize = candidates.iter().map(|candidate| candidate.chars).sum();
    if total_chars <= TOOL_RESULT_BATCH_BUDGET {
        return messages;
    }

    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.chars));
    for candidate in candidates {
        if total_chars <= TOOL_RESULT_BATCH_BUDGET {
            break;
        }
        let Some(content) = tool_result_content_mut(
            &mut messages,
            candidate.message_index,
            candidate.block_index,
        ) else {
            continue;
        };
        let available_chars =
            TOOL_RESULT_BATCH_BUDGET.saturating_sub(total_chars.saturating_sub(candidate.chars));
        let replacement = shortened_batch_tool_result(content, available_chars);
        total_chars = total_chars
            .saturating_sub(candidate.chars)
            .saturating_add(replacement.chars().count());
        *content = replacement;
    }

    messages
}

fn shortened_batch_tool_result(content: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }

    let original_chars = content.chars().count();
    let marker = format!(
        "\n\n[tool_result shortened]\nreason=tool result batch exceeded {TOOL_RESULT_BATCH_BUDGET} chars\noriginal_chars={original_chars}"
    );
    let marker_chars = marker.chars().count();
    if marker_chars >= max_chars {
        return truncate_text(&marker, max_chars);
    }

    let preview_budget = max_chars - marker_chars;
    let preview_head = BATCH_PREVIEW_HEAD.min(preview_budget / 2);
    let preview_tail = BATCH_PREVIEW_TAIL.min(preview_budget.saturating_sub(preview_head));
    let preview = head_tail_text(content, preview_head, preview_tail);
    let preview = truncate_text(&preview, preview_budget);
    format!("{preview}{marker}")
}
